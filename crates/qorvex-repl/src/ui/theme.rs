//! Color theme for the TUI REPL.

use ratatui::style::{Color, Style};

/// Theme colors for consistent styling across the UI.
pub struct Theme;

impl Theme {
    /// Success messages and indicators.
    pub fn success() -> Style {
        Style::default().fg(Color::Green)
    }

    /// Error messages and failure indicators.
    pub fn error() -> Style {
        Style::default().fg(Color::Red)
    }

    /// Element identifiers (accessibility IDs).
    pub fn element_id() -> Style {
        Style::default().fg(Color::Cyan)
    }

    /// Element type names (Button, TextField, etc.).
    pub fn element_type() -> Style {
        Style::default().fg(Color::Yellow)
    }

    /// Element labels (user-visible text).
    pub fn element_label() -> Style {
        Style::default().fg(Color::White)
    }

    /// Element values (text field contents, etc.).
    pub fn element_value() -> Style {
        Style::default().fg(Color::Green)
    }

    /// Muted/secondary text (frames, borders, hints).
    pub fn muted() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Device name styling.
    pub fn device_name() -> Style {
        Style::default().fg(Color::White)
    }

    /// UDID styling.
    pub fn device_udid() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Device state (Booted).
    pub fn device_booted() -> Style {
        Style::default().fg(Color::Green)
    }

    /// Device state (Shutdown).
    pub fn device_shutdown() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Command text in completions.
    pub fn command() -> Style {
        Style::default().fg(Color::Cyan)
    }

    /// Description text in completions.
    pub fn description() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Selected item highlight.
    pub fn selected() -> Style {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    }

    /// Title bar styling.
    pub fn title() -> Style {
        Style::default().fg(Color::Cyan)
    }

    /// Input prompt styling.
    pub fn prompt() -> Style {
        Style::default().fg(Color::Green)
    }

    /// Timestamp styling.
    #[allow(dead_code)]
    pub fn timestamp() -> Style {
        Style::default().fg(Color::DarkGray)
    }
}
