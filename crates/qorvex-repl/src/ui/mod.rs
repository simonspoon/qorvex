//! UI rendering for the TUI REPL.

pub mod completion;
pub mod theme;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
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
    app.output_area = Some(chunks[1]);
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
        Span::styled("[q=quit, Tab=complete, Ctrl+C=copy/quit]", Theme::muted()),
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
    let inner_width = inner.width as usize;
    let viewport_height = inner.height as usize;

    let lines: Vec<Line> = app.output_history.iter().cloned().collect();

    // Calculate total visual lines after wrapping
    let total_visual_lines: usize = lines.iter().map(|line| {
        let w = line.width();
        if w == 0 || inner_width == 0 { 1 } else { (w + inner_width - 1) / inner_width }
    }).sum();

    // Clamp scroll offset and compute scroll position
    let max_scroll = total_visual_lines.saturating_sub(viewport_height);
    app.output_scroll_position = app.output_scroll_position.min(max_scroll);
    let scroll_y = max_scroll.saturating_sub(app.output_scroll_position);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));

    frame.render_widget(paragraph, area);

    // Render selection overlay
    if let Some((sel_start, sel_end)) = app.selection.range() {
        let sel_style = Theme::text_selection();
        let lines_vec: Vec<&Line> = app.output_history.iter().collect();

        // Walk through visual lines to find which screen cells to highlight
        let mut visual_row: usize = 0;
        for (line_idx, line) in lines_vec.iter().enumerate() {
            let line_str: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let w = line.width();
            let wrapped_rows = if w == 0 || inner_width == 0 { 1 } else { (w + inner_width - 1) / inner_width };

            for wrap_row in 0..wrapped_rows {
                let vrow = visual_row + wrap_row;

                // Skip if before scroll viewport
                if vrow < scroll_y {
                    continue;
                }
                let screen_row = vrow - scroll_y;
                if screen_row >= viewport_height {
                    break;
                }

                // Character range for this wrapped row
                let row_char_start = wrap_row * inner_width;
                let row_char_end = ((wrap_row + 1) * inner_width).min(line_str.len());

                // Determine selection overlap on this visual row
                let sel_char_start = if line_idx == sel_start.line {
                    sel_start.col
                } else if line_idx > sel_start.line {
                    0
                } else {
                    continue; // Before selection
                };

                let sel_char_end = if line_idx == sel_end.line {
                    sel_end.col
                } else if line_idx < sel_end.line {
                    line_str.len()
                } else {
                    continue; // After selection
                };

                // Clip to this wrapped row
                let highlight_start = sel_char_start.max(row_char_start);
                let highlight_end = sel_char_end.min(row_char_end);

                if highlight_start < highlight_end {
                    let x = inner.x + (highlight_start - row_char_start) as u16;
                    let y = inner.y + screen_row as u16;
                    let width = (highlight_end - highlight_start) as u16;

                    // Read the existing buffer cells and apply selection style on top
                    for dx in 0..width {
                        if let Some(cell) = frame.buffer_mut().cell_mut(ratatui::layout::Position::new(x + dx, y)) {
                            cell.set_style(sel_style);
                        }
                    }
                }
            }

            visual_row += wrapped_rows;
            if visual_row >= scroll_y + viewport_height {
                break; // Past viewport
            }
        }
    }

    // Render scrollbar if needed
    if total_visual_lines > viewport_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(max_scroll)
            .position(scroll_y);

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
