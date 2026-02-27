use std::io;
use std::path::PathBuf;
use std::time::Duration;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, StatefulImage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use tracing_subscriber::EnvFilter;

use qorvex_core::action::ActionLog;
use qorvex_core::ipc::{IpcClient, IpcResponse};
use qorvex_core::session::SessionEvent;
use qorvex_core::simctl::Simctl;

#[derive(Parser, Debug)]
#[command(name = "qorvex-live")]
#[command(about = "TUI client for monitoring iOS Simulator automation sessions")]
struct Args {
    /// Session name to connect to
    #[arg(short, long, default_value = "default")]
    session: String,

    /// Frames per second for the live video feed (default: 15)
    #[arg(long, default_value_t = 15)]
    fps: u32,

    /// JPEG quality for the live video feed, 1-100 (default: 70)
    #[arg(long, default_value_t = 70)]
    quality: u32,

    /// Disable the live streamer (use polling fallback)
    #[arg(long)]
    no_streamer: bool,

    /// Run in batch mode: print session events as JSONL to stdout and exit
    #[arg(long)]
    batch: bool,

    /// Duration in seconds for batch mode (exit after this many seconds)
    #[arg(long)]
    duration: Option<u64>,
}

/// Maximum number of consecutive IPC connection failures before giving up
const MAX_IPC_RETRIES: u32 = 10;
/// Base delay between retry attempts (will use exponential backoff)
const IPC_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
/// Maximum delay between retry attempts
const IPC_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
enum StreamerStatus {
    Connecting,
    Connected,
    Disconnected,
    NotAvailable(String),
}

/// Message type for internal app events
enum AppEvent {
    SessionEvent(SessionEvent),
    ScreenshotReady(Vec<u8>),
    StreamerFrame(Vec<u8>),
    StreamerStatus(StreamerStatus),
}

struct App {
    action_log: Vec<ActionLog>,
    list_state: ListState,
    current_screenshot: Option<Vec<u8>>,
    session_name: String,
    simulator_udid: Option<String>,
    should_quit: bool,
    streamer_active: bool,
    streamer_status: StreamerStatus,
    image_picker: Picker,
    image_state: Option<StatefulProtocol>,
}

impl App {
    fn new(session_name: String) -> Self {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());

        Self {
            action_log: Vec::new(),
            list_state: ListState::default(),
            current_screenshot: None,
            session_name,
            simulator_udid: Simctl::get_booted_udid().ok(),
            should_quit: false,
            streamer_active: false,
            streamer_status: StreamerStatus::Disconnected,
            image_picker: picker,
            image_state: None,
        }
    }

    fn add_action(&mut self, log: ActionLog) {
        self.action_log.push(log);
        // Auto-scroll to bottom
        self.list_state.select(Some(self.action_log.len().saturating_sub(1)));
    }

    fn update_screenshot(&mut self, base64_png: &str) {
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(base64_png) {
            self.current_screenshot = Some(bytes.clone());
            // Update image state
            if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                self.image_state = Some(self.image_picker.new_resize_protocol(dyn_img));
            }
        }
    }

    fn set_screenshot(&mut self, bytes: Vec<u8>) {
        self.current_screenshot = Some(bytes.clone());
        if let Ok(dyn_img) = image::load_from_memory(&bytes) {
            self.image_state = Some(self.image_picker.new_resize_protocol(dyn_img));
        }
    }
}

/// Spawn a blocking task to capture a screenshot
fn spawn_screenshot_task(udid: String, tx: mpsc::Sender<AppEvent>) {
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            Simctl::screenshot(&udid)
        }).await;

        if let Ok(Ok(bytes)) = result {
            let _ = tx.send(AppEvent::ScreenshotReady(bytes)).await;
        }
    });
}

fn spawn_streamer_task(
    session_name: &str,
    udid: &str,
    fps: u32,
    quality: u32,
    tx: mpsc::Sender<AppEvent>,
    cancel: CancellationToken,
) {
    let socket_dir = dirs::home_dir()
        .expect("home dir")
        .join(".qorvex");
    std::fs::create_dir_all(&socket_dir).ok();
    let socket_path = socket_dir.join(format!("streamer_{}.sock", session_name));

    // Clean up stale socket
    let _ = std::fs::remove_file(&socket_path);

    let socket_path_str = socket_path.to_string_lossy().to_string();
    let udid = udid.to_string();

    tokio::spawn(async move {
        // Find the qorvex-streamer binary
        let streamer_bin = which_streamer();
        let Some(bin_path) = streamer_bin else {
            let _ = tx.send(AppEvent::StreamerStatus(
                StreamerStatus::NotAvailable("qorvex-streamer binary not found".into())
            )).await;
            return;
        };

        tracing::info!(path = %bin_path.display(), "found qorvex-streamer binary");
        let _ = tx.send(AppEvent::StreamerStatus(StreamerStatus::Connecting)).await;

        // Spawn the streamer process with stdio redirected to avoid TUI corruption
        use std::process::Stdio;
        let mut child = match tokio::process::Command::new(&bin_path)
            .arg("--socket-path").arg(&socket_path_str)
            .arg("--udid").arg(&udid)
            .arg("--fps").arg(fps.to_string())
            .arg("--quality").arg(quality.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppEvent::StreamerStatus(
                    StreamerStatus::NotAvailable(format!("Failed to spawn streamer: {e}"))
                )).await;
                return;
            }
        };

        // Wait for socket to appear, then connect (timeout after 10s)
        let connect_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let stream = loop {
            if cancel.is_cancelled() {
                let _ = child.kill().await;
                return;
            }

            if tokio::time::Instant::now() >= connect_deadline {
                tracing::warn!("timed out connecting to streamer socket");
                let stderr_msg = read_child_stderr(&mut child).await;
                let msg = if stderr_msg.is_empty() {
                    "Streamer connection timed out".into()
                } else {
                    format!("Streamer failed: {stderr_msg}")
                };
                let _ = child.kill().await;
                let _ = tx.send(AppEvent::StreamerStatus(
                    StreamerStatus::NotAvailable(msg)
                )).await;
                return;
            }

            // Check if child has exited
            if let Ok(Some(status)) = child.try_wait() {
                let stderr_msg = read_child_stderr(&mut child).await;
                let msg = if status.code() == Some(2) {
                    "Screen recording permission required. Grant in System Settings > Privacy > Screen Recording".into()
                } else if !stderr_msg.is_empty() {
                    stderr_msg
                } else {
                    format!("Streamer exited with status: {status}")
                };
                tracing::warn!(msg, "streamer process exited");
                let _ = tx.send(AppEvent::StreamerStatus(
                    StreamerStatus::NotAvailable(msg)
                )).await;
                return;
            }

            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(s) => break s,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        };

        let _ = tx.send(AppEvent::StreamerStatus(StreamerStatus::Connected)).await;

        // Read frame loop
        let mut reader = tokio::io::BufReader::new(stream);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return;
                }
                result = read_frame(&mut reader) => {
                    match result {
                        Ok(bytes) => {
                            let _ = tx.send(AppEvent::StreamerFrame(bytes)).await;
                        }
                        Err(_) => {
                            let _ = tx.send(AppEvent::StreamerStatus(StreamerStatus::Disconnected)).await;
                            let _ = child.kill().await;
                            return;
                        }
                    }
                }
            }
        }
    });
}

async fn read_child_stderr(child: &mut tokio::process::Child) -> String {
    use tokio::io::AsyncReadExt;
    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            stderr.read_to_end(&mut buf),
        ).await;
        let s = String::from_utf8_lossy(&buf).trim().to_string();
        // Return the last meaningful line
        s.lines()
            .rev()
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    }
}

async fn read_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let len = reader.read_u32_le().await? as usize;
    if len == 0 || len > 10_000_000 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid frame length"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

fn which_streamer() -> Option<PathBuf> {
    // Check PATH
    if let Ok(output) = std::process::Command::new("which")
        .arg("qorvex-streamer")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path.into());
            }
        }
    }
    // Check next to our own binary (e.g. both in ~/.cargo/bin/)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join("qorvex-streamer");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    // Check the repo's Swift build directories (for development)
    // Walk up from the current exe to find the workspace root containing qorvex-streamer/
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        while let Some(d) = dir {
            for profile in ["release", "debug"] {
                let candidate = d.join("qorvex-streamer").join(".build").join(profile).join("qorvex-streamer");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
        }
    }
    // Also check relative to the current working directory
    if let Ok(cwd) = std::env::current_dir() {
        for profile in ["release", "debug"] {
            let candidate = cwd.join("qorvex-streamer").join(".build").join(profile).join("qorvex-streamer");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Run in batch mode: connect to IPC, print session events as JSONL to stdout, exit after duration.
async fn run_batch(args: Args) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let session_name = &args.session;
    let duration = args.duration.map(Duration::from_secs);

    // Connect to IPC
    let mut client = match qorvex_core::ipc::IpcClient::connect(session_name).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to server for session '{}': {}", session_name, e);
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()));
        }
    };

    // Subscribe to events
    if let Err(e) = client.subscribe().await {
        eprintln!("Failed to subscribe to events: {}", e);
        return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
    }

    eprintln!("Connected to session '{}', streaming events...", session_name);

    let mut stdout = tokio::io::stdout();
    let deadline = duration.map(|d| tokio::time::Instant::now() + d);

    loop {
        let timeout_fut = async {
            if let Some(dl) = deadline {
                tokio::time::sleep_until(dl).await;
            } else {
                // No deadline — sleep forever (cancelled by other branches)
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            result = client.read_event() => {
                match result {
                    Ok(IpcResponse::Event { event }) => {
                        match serde_json::to_string(&event) {
                            Ok(json) => {
                                let line = format!("{}\n", json);
                                if stdout.write_all(line.as_bytes()).await.is_err() {
                                    break; // stdout closed
                                }
                                let _ = stdout.flush().await;
                            }
                            Err(e) => {
                                eprintln!("Failed to serialize event: {}", e);
                            }
                        }
                    }
                    Ok(_) => {} // ignore non-event responses
                    Err(e) => {
                        eprintln!("IPC error: {}", e);
                        break;
                    }
                }
            }
            _ = timeout_fut => {
                eprintln!("Duration elapsed, exiting.");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("Interrupted, exiting.");
                break;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let log_dir = qorvex_core::session::logs_dir();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "qorvex-live.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    let args = Args::parse();

    if args.batch {
        return run_batch(args).await;
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(args.session);

    // Channel for all app events (IPC events and screenshot results)
    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(100);

    // Initial screenshot (non-blocking)
    if let Some(udid) = app.simulator_udid.clone() {
        spawn_screenshot_task(udid, event_tx.clone());
    }

    // Create cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();
    let ipc_cancel = cancel_token.clone();

    // Try to connect to IPC
    let session_name = app.session_name.clone();
    let ipc_tx = event_tx.clone();

    tokio::spawn(async move {
        let mut retry_count: u32 = 0;

        loop {
            // Check for cancellation before attempting connection
            if ipc_cancel.is_cancelled() {
                break;
            }

            match IpcClient::connect(&session_name).await {
                Ok(mut client) => {
                    // Reset retry count on successful connection
                    retry_count = 0;

                    if client.subscribe().await.is_ok() {
                        loop {
                            tokio::select! {
                                _ = ipc_cancel.cancelled() => {
                                    break;
                                }
                                result = client.read_event() => {
                                    match result {
                                        Ok(IpcResponse::Event { event }) => {
                                            if ipc_tx.send(AppEvent::SessionEvent(event)).await.is_err() {
                                                break;
                                            }
                                        }
                                        Err(_) => break,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    retry_count += 1;
                    if retry_count >= MAX_IPC_RETRIES {
                        // Stop retrying after max attempts
                        break;
                    }
                }
            }

            // Check for cancellation before sleeping
            if ipc_cancel.is_cancelled() {
                break;
            }

            // Exponential backoff: delay = base * 2^(retry_count - 1), capped at max
            let backoff_multiplier = 2u64.saturating_pow(retry_count.saturating_sub(1));
            let delay = IPC_RETRY_BASE_DELAY
                .saturating_mul(backoff_multiplier as u32)
                .min(IPC_RETRY_MAX_DELAY);

            tokio::select! {
                _ = ipc_cancel.cancelled() => {
                    break;
                }
                _ = tokio::time::sleep(delay) => {}
            }
        }
    });

    // Start streamer if available
    if !args.no_streamer {
        if let Some(ref udid) = app.simulator_udid {
            spawn_streamer_task(
                &app.session_name,
                udid,
                args.fps,
                args.quality,
                event_tx.clone(),
                cancel_token.clone(),
            );
        }
    }

    // Main loop
    loop {
        // Check for app events (IPC events and screenshot results)
        while let Ok(app_event) = event_rx.try_recv() {
            match app_event {
                AppEvent::SessionEvent(event) => match event {
                    SessionEvent::ActionLogged(log) => {
                        if let Some(ref ss) = log.screenshot {
                            app.update_screenshot(ss);
                        }
                        app.add_action(log);
                    }
                    SessionEvent::ScreenshotUpdated(ss) => {
                        app.update_screenshot(&ss);
                    }
                    _ => {}
                },
                AppEvent::ScreenshotReady(bytes) => {
                    app.set_screenshot(bytes);
                }
                AppEvent::StreamerFrame(bytes) => {
                    app.set_screenshot(bytes);
                    app.streamer_active = true;
                }
                AppEvent::StreamerStatus(status) => {
                    app.streamer_status = status;
                    if matches!(app.streamer_status, StreamerStatus::Disconnected | StreamerStatus::NotAvailable(_)) {
                        app.streamer_active = false;
                    }
                }
            }
        }

        terminal.draw(|f| ui(f, &mut app))?;

        // Poll for events with timeout for responsiveness
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => {
                            // Cancel the IPC task before quitting
                            cancel_token.cancel();
                            app.should_quit = true;
                        }
                        KeyCode::Char('r') => {
                            // Trigger non-blocking screenshot refresh
                            if let Some(udid) = app.simulator_udid.clone() {
                                spawn_screenshot_task(udid, event_tx.clone());
                            }
                        }
                        KeyCode::Up => {
                            let i = app.list_state.selected().unwrap_or(0);
                            app.list_state.select(Some(i.saturating_sub(1)));
                        }
                        KeyCode::Down => {
                            let i = app.list_state.selected().unwrap_or(0);
                            let max = app.action_log.len().saturating_sub(1);
                            app.list_state.select(Some((i + 1).min(max)));
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    // Split into left (simulator) and right (log)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(f.area());

    // Left: Simulator screenshot
    let sim_title = match &app.streamer_status {
        StreamerStatus::Connected => " Simulator (live) ".to_string(),
        StreamerStatus::Connecting => " Simulator (connecting...) ".to_string(),
        StreamerStatus::Disconnected => " Simulator ".to_string(),
        StreamerStatus::NotAvailable(reason) => format!(" Simulator ({reason}) "),
    };
    let sim_block = Block::default()
        .title(sim_title.as_str())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.streamer_active { Color::Green } else { Color::Yellow }));

    let inner = sim_block.inner(chunks[0]);
    f.render_widget(sim_block, chunks[0]);

    if let Some(ref mut state) = app.image_state {
        let image = StatefulImage::default();
        f.render_stateful_widget(image, inner, state);
    } else {
        let placeholder = Paragraph::new("No screenshot")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(placeholder, inner);
    }

    // Right: Action log
    let log_block = Block::default()
        .title(" Action Log (q=quit, r=refresh, arrow-up/down=scroll) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner_width = log_block.inner(chunks[1]).width as usize;

    let items: Vec<ListItem> = app.action_log.iter().map(|log| {
        let timestamp = log.timestamp.format("%H:%M:%S%.3f").to_string();
        let action_desc = format!("{:?}", log.action);
        let result = match &log.result {
            qorvex_core::action::ActionResult::Success => "success",
            qorvex_core::action::ActionResult::Failure(e) => e.as_str(),
        };
        let has_screenshot = if log.screenshot.is_some() { " [img]" } else { "" };

        let header = Line::from(vec![
            Span::styled(timestamp, Style::default().fg(Color::Yellow)),
            Span::raw(" -> "),
            Span::styled(
                result,
                Style::default().fg(if result == "success" { Color::Green } else { Color::Red }),
            ),
            Span::raw(has_screenshot),
        ]);

        let indent = "  ";
        let wrap_width = inner_width.saturating_sub(indent.len()).max(1);
        let mut lines = vec![header];

        let mut remaining = action_desc.as_str();
        while !remaining.is_empty() {
            let (chunk, rest) = if remaining.len() > wrap_width {
                let break_at = remaining[..wrap_width]
                    .rfind(|c: char| c == ' ' || c == ',' || c == '{' || c == '}')
                    .map(|i| i + 1)
                    .unwrap_or(wrap_width);
                (&remaining[..break_at], &remaining[break_at..])
            } else {
                (remaining, "")
            };
            lines.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(chunk.to_string(), Style::default().fg(Color::White)),
            ]));
            remaining = rest;
        }

        ListItem::new(Text::from(lines))
    }).collect();

    let list = List::new(items)
        .block(log_block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_defaults() {
        let args = Args::parse_from(["qorvex-live"]);
        assert_eq!(args.session, "default");
        assert!(!args.batch);
        assert!(args.duration.is_none());
        assert!(!args.no_streamer);
    }

    #[test]
    fn test_args_batch_mode() {
        let args = Args::parse_from(["qorvex-live", "--batch", "--duration", "5", "-s", "test"]);
        assert!(args.batch);
        assert_eq!(args.duration, Some(5));
        assert_eq!(args.session, "test");
    }
}
