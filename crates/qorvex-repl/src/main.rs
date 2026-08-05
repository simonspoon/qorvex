//! TUI REPL for iOS Simulator automation.

mod app;
mod completion;
mod format;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            KeyModifiers, MouseButton, MouseEventKind,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    Terminal,
};
use tracing_subscriber::EnvFilter;
use tui_input::backend::crossterm::EventHandler;

use app::App;

#[derive(Parser, Debug)]
#[command(name = "qorvex-repl")]
#[command(about = "Interactive TUI REPL for iOS Simulator automation")]
struct Args {
    /// Session name for IPC socket
    #[arg(short, long, default_value = "default")]
    session: String,

    /// Run in batch mode: read commands from stdin, print results to stdout
    #[arg(long)]
    batch: bool,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let log_dir = qorvex_core::session::logs_dir();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "qorvex-repl.log");
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
        return run_batch(args.session).await;
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app (fast — no blocking I/O)
    let mut app = App::new(args.session);

    // Main loop
    let result = run_app(&mut terminal, &mut app).await;

    // Shut down the server so it removes its socket file
    app.shutdown().await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

/// Convert mouse (column, row) screen coordinates to a TextPosition in the output buffer.
fn mouse_to_text_position(column: u16, row: u16, app: &mut App) -> Option<app::TextPosition> {
    let area = app.output_area?;

    // Inner area (inside borders)
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    // Check bounds
    if column < inner_x || row < inner_y {
        return None;
    }
    let rel_col = (column - inner_x) as usize;
    let rel_row = (row - inner_y) as usize;
    if rel_col >= inner_width || rel_row >= inner_height {
        return None;
    }

    // Calculate which visual line corresponds to this row, accounting for scroll
    app.ensure_visual_layout(inner_width);
    let total_visual_lines = app.total_visual_lines();

    let max_scroll = total_visual_lines.saturating_sub(inner_height);
    let scroll_y = max_scroll.saturating_sub(app.output_scroll_position);

    let target_visual_row = scroll_y + rel_row;

    // Last logical line starting at or before the target row.
    let starts = app.visual_layout();
    let line_count = app.output_history.len();
    let line_idx = starts.partition_point(|&row| row <= target_visual_row) - 1;

    if line_idx < line_count {
        let line = &app.output_history[line_idx];
        let line_str: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let row_within_line = target_visual_row - starts[line_idx];
        let col = row_within_line * inner_width + rel_col;
        let col = col.min(line_str.len());
        return Some(app::TextPosition::new(line_idx, col));
    }

    // Past the end — clamp to last line
    if let Some(last) = app.output_history.back() {
        let line_str: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
        Some(app::TextPosition::new(line_count - 1, line_str.len()))
    } else {
        None
    }
}

/// Run in batch mode: read commands line-by-line from stdin, print results to stdout.
///
/// No terminal setup (raw mode, alternate screen). Connects to IPC server,
/// processes each line as a command, prints output as plain text, exits on EOF.
async fn run_batch(session: String) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut app = App::new_blocking(session).await;

    // Drain and print startup messages
    for line in app.take_output() {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        println!("{}", text);
    }

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "quit" || line == "exit" {
            break;
        }

        app.process_command(&line).await;

        // Drain output and print as plain text
        for output_line in app.take_output() {
            let text: String = output_line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            println!("{}", text);
        }

        if app.should_quit {
            break;
        }
    }

    // Shut down the server so it removes its socket file
    app.shutdown().await;

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    // Render the first frame, then kick off background startup
    terminal.draw(|f| ui::render(f, app))?;
    app.startup();

    loop {
        // Check for background task completions
        app.check_startup_result();
        app.check_element_updates();
        app.check_app_updates();
        app.check_command_result();

        // Render
        terminal.draw(|f| ui::render(f, app))?;

        // Poll for events with timeout
        let poll_timeout = if app.is_processing {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(100)
        };
        if event::poll(poll_timeout)? {
            handle_event(app, event::read()?);

            // Consume everything already queued so a burst of keystrokes costs
            // one draw, not one per key. Bounded so a mouse-drag storm cannot
            // starve rendering.
            let drain_start = Instant::now();
            let mut drained = 0;
            while !app.should_quit
                && drained < MAX_DRAINED_EVENTS
                && drain_start.elapsed() < MAX_DRAIN_TIME
                && event::poll(Duration::ZERO)?
            {
                handle_event(app, event::read()?);
                drained += 1;
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Upper bound on events consumed before forcing a redraw.
const MAX_DRAINED_EVENTS: usize = 256;
/// Upper bound on time spent draining events before forcing a redraw.
const MAX_DRAIN_TIME: Duration = Duration::from_millis(10);

/// Apply a single terminal event to the app state.
fn handle_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key) => {
            if key.kind != KeyEventKind::Press {
                return;
            }

            // Ctrl+C: copy if selection active, otherwise quit
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                if app.selection.has_selection() {
                    app.copy_selection_to_clipboard();
                } else {
                    app.should_quit = true;
                }
            }
            // Any keypress clears selection (except Ctrl+C which already handled it)
            else {
                app.selection.clear();

                // Handle completion navigation
                if app.completion.visible {
                    match key.code {
                        KeyCode::Tab | KeyCode::Enter => {
                            app.accept_completion();
                        }
                        KeyCode::Up => {
                            app.completion.select_prev();
                        }
                        KeyCode::Down => {
                            app.completion.select_next();
                        }
                        KeyCode::Esc => {
                            app.completion.hide();
                        }
                        _ => {
                            // Pass through to input handler
                            app.input.handle_event(&Event::Key(key));
                            app.update_completion();
                        }
                    }
                }
                // Handle normal input
                else {
                    match key.code {
                        KeyCode::Enter => {
                            if !app.is_processing {
                                app.execute_command();
                            }
                        }
                        KeyCode::Char('q') if app.input.value().is_empty() => {
                            app.should_quit = true;
                        }
                        KeyCode::Up => {
                            app.scroll_up();
                        }
                        KeyCode::Down => {
                            app.scroll_down();
                        }
                        KeyCode::Tab => {
                            app.update_completion();
                        }
                        KeyCode::Esc => {
                            // Clear input
                            app.input = tui_input::Input::default();
                        }
                        _ => {
                            app.input.handle_event(&Event::Key(key));
                            app.update_completion();
                        }
                    }
                }
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                    app.selection.clear();
                    app.selection.anchor = Some(pos);
                    app.selection.dragging = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if app.selection.dragging => {
                if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                    app.selection.endpoint = Some(pos);
                }
            }
            MouseEventKind::Up(MouseButton::Left) if app.selection.dragging => {
                if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                    app.selection.endpoint = Some(pos);
                }
                app.selection.dragging = false;
            }
            MouseEventKind::ScrollUp => {
                app.scroll_up();
            }
            MouseEventKind::ScrollDown => {
                app.scroll_down();
            }
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_batch_flag() {
        let args = Args::parse_from(["qorvex-repl", "--batch", "-s", "test"]);
        assert!(args.batch);
        assert_eq!(args.session, "test");
    }

    #[test]
    fn test_args_default_no_batch() {
        let args = Args::parse_from(["qorvex-repl"]);
        assert!(!args.batch);
    }

    #[test]
    fn test_args_parsing() {
        let args = Args::parse_from(["qorvex-repl"]);
        assert_eq!(args.session, "default");

        let args = Args::parse_from(["qorvex-repl", "--session", "test"]);
        assert_eq!(args.session, "test");

        let args = Args::parse_from(["qorvex-repl", "-s", "custom"]);
        assert_eq!(args.session, "custom");
    }
}
