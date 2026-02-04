//! UI rendering for the TUI REPL.

pub mod completion;
pub mod theme;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::app::App;
use crate::ui::completion::CompletionPopup;
use crate::ui::theme::Theme;

/// Render the main UI.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Split into title, output, and input
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title bar
            Constraint::Min(5),     // Output history
            Constraint::Length(3),  // Input line
        ])
        .split(area);

    render_title(frame, app, chunks[0]);
    render_output(frame, app, chunks[1]);
    render_input(frame, app, chunks[2]);

    // Render completion popup if visible
    if app.completion.visible {
        render_completion(frame, app, chunks[2]);
    }
}

fn render_title(frame: &mut Frame, app: &App, area: Rect) {
    let session_info = if app.session.is_some() {
        format!("session: {}", app.session_name)
    } else {
        "no session".to_string()
    };

    let device_info = app.simulator_udid
        .as_ref()
        .map(|u| format!("device: {}...", &u[..8]))
        .unwrap_or_else(|| "no device".to_string());

    let title = Line::from(vec![
        Span::styled(" qorvex-repl ", Theme::title().add_modifier(Modifier::BOLD)),
        Span::styled(format!("({}) ", session_info), Theme::muted()),
        Span::styled(format!("[{}] ", device_info), Theme::muted()),
        Span::styled("[q=quit, Tab=complete]", Theme::muted()),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Theme::title());

    let paragraph = Paragraph::new(title).block(block);
    frame.render_widget(paragraph, area);
}

fn render_output(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Output ")
        .borders(Borders::ALL)
        .border_style(Theme::muted());

    let inner = block.inner(area);

    // Convert output lines to ListItems
    let items: Vec<ListItem> = app.output_history
        .iter()
        .map(|line| ListItem::new(line.clone()))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Theme::selected());

    frame.render_stateful_widget(list, area, &mut app.output_scroll_state);

    // Render scrollbar if needed
    if app.output_history.len() > inner.height as usize {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(app.output_history.len())
            .position(app.output_scroll_position);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
            &mut scrollbar_state,
        );
    }
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(" Input ")
        .borders(Borders::ALL)
        .border_style(Theme::prompt());

    let inner = block.inner(area);

    let input_text = app.input.value();
    let input_line = Line::from(vec![
        Span::styled("> ", Theme::prompt()),
        Span::raw(input_text),
    ]);

    let paragraph = Paragraph::new(input_line).block(block);
    frame.render_widget(paragraph, area);

    // Position cursor
    let cursor_x = inner.x + 2 + app.input.visual_cursor() as u16;
    let cursor_y = inner.y;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_completion(frame: &mut Frame, app: &App, input_area: Rect) {
    let popup = CompletionPopup::new(&app.completion);

    // Position popup above the input area
    let cursor_x = input_area.x + 2 + app.input.visual_cursor() as u16;
    let cursor_y = input_area.y;

    let popup_area = popup.area(cursor_x, cursor_y, frame.area());
    frame.render_widget(popup, popup_area);
}
