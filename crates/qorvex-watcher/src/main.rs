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

use qorvex_core::action::ActionLog;
use qorvex_core::ipc::{IpcClient, IpcResponse};
use qorvex_core::session::SessionEvent;
use qorvex_core::simctl::Simctl;

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

    fn refresh_screenshot(&mut self) {
        if let Some(udid) = &self.simulator_udid {
            if let Ok(bytes) = Simctl::screenshot(udid) {
                self.current_screenshot = Some(bytes.clone());
                if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                    self.image_state = Some(self.image_picker.new_resize_protocol(dyn_img));
                }
            }
        }
    }
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

    // Initial screenshot
    app.refresh_screenshot();

    // Try to connect to IPC
    let (event_tx, mut event_rx) = mpsc::channel::<SessionEvent>(100);
    let session_name = app.session_name.clone();

    tokio::spawn(async move {
        // Keep trying to connect
        loop {
            if let Ok(mut client) = IpcClient::connect(&session_name).await {
                if client.subscribe().await.is_ok() {
                    loop {
                        match client.read_event().await {
                            Ok(IpcResponse::Event { event }) => {
                                if event_tx.send(event).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                            _ => {}
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Main loop
    loop {
        // Check for IPC events
        while let Ok(event) = event_rx.try_recv() {
            match event {
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
            }
        }

        terminal.draw(|f| ui(f, &mut app))?;

        // Poll for events with timeout for responsiveness
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => app.should_quit = true,
                        KeyCode::Char('r') => app.refresh_screenshot(),
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
