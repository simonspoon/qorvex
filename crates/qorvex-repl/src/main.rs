//! TUI REPL for iOS Simulator automation.

mod app;
mod completion;
mod format;
mod ui;

use std::io;
use std::time::Duration;

use clap::Parser;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind, MouseButton},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    Terminal,
};
use tui_input::backend::crossterm::EventHandler;

use app::App;

#[derive(Parser, Debug)]
#[command(name = "qorvex-repl")]
#[command(about = "Interactive TUI REPL for iOS Simulator automation")]
struct Args {
    /// Session name for IPC socket
    #[arg(short, long, default_value = "default")]
    session: String,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(args.session);

    // Main loop
    let result = run_app(&mut terminal, &mut app).await;

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
fn mouse_to_text_position(
    column: u16,
    row: u16,
    app: &App,
) -> Option<app::TextPosition> {
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
    let lines: Vec<&ratatui::text::Line> = app.output_history.iter().collect();

    // Build a map of visual rows to (logical_line, char_offset)
    let total_visual_lines: usize = lines.iter().map(|line| {
        let w = line.width();
        if w == 0 || inner_width == 0 { 1 } else { (w + inner_width - 1) / inner_width }
    }).sum();

    let max_scroll = total_visual_lines.saturating_sub(inner_height);
    let scroll_y = max_scroll.saturating_sub(app.output_scroll_position);

    let target_visual_row = scroll_y + rel_row;

    let mut visual_row = 0;
    for (line_idx, line) in lines.iter().enumerate() {
        let line_str: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let w = line.width();
        let wrapped_rows = if w == 0 || inner_width == 0 { 1 } else { (w + inner_width - 1) / inner_width };

        if target_visual_row < visual_row + wrapped_rows {
            // This is the line
            let row_within_line = target_visual_row - visual_row;
            let col = row_within_line * inner_width + rel_col;
            let col = col.min(line_str.len());
            return Some(app::TextPosition::new(line_idx, col));
        }
        visual_row += wrapped_rows;
    }

    // Past the end — clamp to last line
    if let Some(last) = lines.last() {
        let line_str: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
        Some(app::TextPosition::new(lines.len() - 1, line_str.len()))
    } else {
        None
    }
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Check for element updates from watcher
        app.check_element_updates();

        // Render
        terminal.draw(|f| ui::render(f, app))?;

        // Poll for events with timeout
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
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
                                    app.execute_command().await;
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
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                                app.selection.clear();
                                app.selection.anchor = Some(pos);
                                app.selection.dragging = true;
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if app.selection.dragging {
                                if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                                    app.selection.endpoint = Some(pos);
                                }
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if app.selection.dragging {
                                if let Some(pos) = mouse_to_text_position(mouse.column, mouse.row, app) {
                                    app.selection.endpoint = Some(pos);
                                }
                                app.selection.dragging = false;
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            app.scroll_up();
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll_down();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
