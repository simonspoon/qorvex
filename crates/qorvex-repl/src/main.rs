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
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
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

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Render
        terminal.draw(|f| ui::render(f, app))?;

        // Poll for events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Handle Ctrl+C for quit
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                }
                // Handle completion navigation
                else if app.completion.visible {
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
