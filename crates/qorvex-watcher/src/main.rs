use std::io;
use std::time::Duration;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, StatefulImage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use qorvex_core::action::ActionLog;
use qorvex_core::ipc::{IpcClient, IpcResponse};
use qorvex_core::session::SessionEvent;
use qorvex_core::simctl::Simctl;

/// Maximum number of consecutive IPC connection failures before giving up
const MAX_IPC_RETRIES: u32 = 10;
/// Base delay between retry attempts (will use exponential backoff)
const IPC_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
/// Maximum delay between retry attempts
const IPC_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

/// Message type for internal app events
enum AppEvent {
    SessionEvent(SessionEvent),
    ScreenshotReady(Vec<u8>),
}

struct App {
    action_log: Vec<ActionLog>,
    list_state: ListState,
    current_screenshot: Option<Vec<u8>>,
    session_name: String,
    simulator_udid: Option<String>,
    should_quit: bool,
    image_picker: Picker,
    image_state: Option<StatefulProtocol>,
}

impl App {
    fn new() -> Self {
        let picker = Picker::from_query_stdio()
            .unwrap_or_else(|_| Picker::from_fontsize((8, 16)));

        Self {
            action_log: Vec::new(),
            list_state: ListState::default(),
            current_screenshot: None,
            session_name: "default".to_string(),
            simulator_udid: Simctl::get_booted_udid().ok(),
            should_quit: false,
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

#[tokio::main]
async fn main() -> io::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

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
    let sim_block = Block::default()
        .title(" Simulator ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

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

    let items: Vec<ListItem> = app.action_log.iter().map(|log| {
        let timestamp = log.timestamp.format("%H:%M:%S%.3f").to_string();
        let action_desc = format!("{:?}", log.action);
        let result = match &log.result {
            qorvex_core::action::ActionResult::Success => "success",
            qorvex_core::action::ActionResult::Failure(e) => e.as_str(),
        };
        let has_screenshot = if log.screenshot.is_some() { "[img]" } else { "" };

        let line = Line::from(vec![
            Span::styled(timestamp, Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::styled(action_desc, Style::default().fg(Color::White)),
            Span::raw(" -> "),
            Span::styled(result, Style::default().fg(if result == "success" { Color::Green } else { Color::Red })),
            Span::raw(" "),
            Span::raw(has_screenshot),
        ]);

        ListItem::new(line)
    }).collect();

    let list = List::new(items)
        .block(log_block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);
}
