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
            Constraint::Length(3), // Title bar
            Constraint::Min(5),    // Output history
            Constraint::Length(3), // Input line
        ])
        .split(area);

    render_title(frame, app, chunks[0]);
    app.output_area = Some(chunks[1]);
    render_output(frame, app, chunks[1]);
    render_input(frame, app, chunks[2]);

    // Render completion popup if visible (not during processing)
    if app.completion.visible && !app.is_processing {
        render_completion(frame, app, chunks[2]);
    }
}

fn render_title(frame: &mut Frame, app: &App, area: Rect) {
    let session_info = format!("session: {}", app.session_name);

    let device_info = "ipc-client".to_string();

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

/// Half-open range of logical line indices whose visual rows intersect
/// `[scroll_y, scroll_y + viewport_height)`.
///
/// `starts` is `App::visual_layout()`: strictly increasing, one entry per
/// logical line plus a total.
fn visible_line_range(starts: &[usize], scroll_y: usize, viewport_height: usize) -> (usize, usize) {
    let line_count = starts.len().saturating_sub(1);
    // Last line starting at or before scroll_y (starts[0] == 0, so this is >= 1).
    let first = starts.partition_point(|&row| row <= scroll_y) - 1;
    let first = first.min(line_count);
    // Lines starting before the bottom of the viewport.
    let last = starts
        .partition_point(|&row| row < scroll_y + viewport_height)
        .min(line_count);
    (first, last.max(first))
}

fn render_output(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Output ")
        .borders(Borders::ALL)
        .border_style(Theme::muted());

    let inner = block.inner(area);
    let inner_width = inner.width as usize;
    let viewport_height = inner.height as usize;

    app.ensure_visual_layout(inner_width);
    let total_visual_lines = app.total_visual_lines();

    // Clamp scroll offset and compute scroll position
    let max_scroll = total_visual_lines.saturating_sub(viewport_height);
    app.output_scroll_position = app.output_scroll_position.min(max_scroll);
    let scroll_y = max_scroll.saturating_sub(app.output_scroll_position);

    // Only the logical lines whose visual rows intersect the viewport need to be
    // cloned and handed to Paragraph — the rest would just be re-wrapped and
    // discarded, which is what made typing lag once history filled up.
    let (first, last) = visible_line_range(app.visual_layout(), scroll_y, viewport_height);
    let lines: Vec<Line> = app
        .output_history
        .iter()
        .skip(first)
        .take(last - first)
        .cloned()
        .collect();
    // Residual scroll within the first rendered line.
    let inner_scroll = scroll_y - app.visual_layout()[first];

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((inner_scroll as u16, 0));

    frame.render_widget(paragraph, area);

    // Render selection overlay
    if let Some((sel_start, sel_end)) = app.selection.range() {
        let sel_style = Theme::text_selection();
        let starts = app.visual_layout();

        // Walk the visible logical lines to find which screen cells to highlight
        for (line_idx, line) in app
            .output_history
            .iter()
            .enumerate()
            .skip(first)
            .take(last - first)
        {
            let line_str: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let visual_row = starts[line_idx];
            let wrapped_rows = crate::app::wrapped_rows(line, inner_width);

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
                        if let Some(cell) = frame
                            .buffer_mut()
                            .cell_mut(ratatui::layout::Position::new(x + dx, y))
                        {
                            cell.set_style(sel_style);
                        }
                    }
                }
            }
        }
    }

    // Render scrollbar if needed
    if total_visual_lines > viewport_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll_y);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
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

    if app.is_processing {
        let spinner = app.spinner_frame();
        let input_line = Line::from(vec![
            Span::styled(format!("{} ", spinner), Theme::prompt()),
            Span::styled(&app.processing_label, Theme::muted()),
            Span::styled(" …", Theme::muted()),
        ]);
        let paragraph = Paragraph::new(input_line).block(block);
        frame.render_widget(paragraph, area);
    } else {
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
}

fn render_completion(frame: &mut Frame, app: &App, input_area: Rect) {
    let popup = CompletionPopup::new(&app.completion);

    // Position popup above the input area
    let cursor_x = input_area.x + 2 + app.input.visual_cursor() as u16;
    let cursor_y = input_area.y;

    let popup_area = popup.area(cursor_x, cursor_y, frame.area());
    frame.render_widget(popup, popup_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;
    use std::time::{Duration, Instant};

    fn app_with_history(count: usize) -> App {
        let mut app = App::new("perf-test".to_string());
        for i in 0..count {
            app.add_output(Line::from(format!("line {:04} {}", i, "x ".repeat(56))));
        }
        app
    }

    #[test]
    fn test_visible_line_range() {
        // Three logical lines occupying 1, 3 and 1 visual rows.
        let starts = [0, 1, 4, 5];

        // Whole history fits in the viewport.
        assert_eq!(visible_line_range(&starts, 0, 10), (0, 3));
        // Viewport starting mid-way through line 1.
        assert_eq!(visible_line_range(&starts, 2, 2), (1, 2));
        // Viewport covering the tail of line 1 and line 2.
        assert_eq!(visible_line_range(&starts, 3, 2), (1, 3));
        // Empty history.
        assert_eq!(visible_line_range(&[0], 0, 10), (0, 0));
        // Degenerate viewport.
        assert_eq!(visible_line_range(&starts, 4, 0), (2, 2));
    }

    /// A full scrollback must not cost a full-history render per frame.
    #[tokio::test]
    async fn test_render_full_history_is_fast() {
        let mut app = app_with_history(1000);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        let start = Instant::now();
        for _ in 0..200 {
            terminal.draw(|f| render(f, &mut app)).unwrap();
        }
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "200 frames over a 1000-line history took {:?}",
            elapsed
        );
    }

    /// The selection overlay must still land on the right logical lines now
    /// that it walks only the sliced range.
    #[tokio::test]
    async fn test_selection_overlay_lands_on_selected_lines() {
        let mut app = app_with_history(200);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        // First render establishes output_area / the visual layout.
        terminal.draw(|f| render(f, &mut app)).unwrap();

        // Select the whole of the second-to-last logical line.
        let last_idx = app.output_history.len() - 1;
        app.selection.anchor = Some(crate::app::TextPosition::new(last_idx - 1, 0));
        app.selection.endpoint = Some(crate::app::TextPosition::new(last_idx, 0));
        terminal.draw(|f| render(f, &mut app)).unwrap();

        let area = app.output_area.unwrap();
        let buffer = terminal.backend().buffer();
        let selected_bg = Theme::text_selection().bg.unwrap();

        // Rows holding the selected line are highlighted; the last line is not.
        let starts = app.visual_layout();
        let scroll_y = starts[starts.len() - 1] - (area.height.saturating_sub(2) as usize);
        for (idx, expected) in [(last_idx - 1, true), (last_idx, false)] {
            let screen_row = area.y + 1 + (starts[idx] - scroll_y) as u16;
            let cell = buffer.cell(Position::new(area.x + 1, screen_row)).unwrap();
            assert_eq!(
                cell.bg == selected_bg,
                expected,
                "line {} at row {} highlight mismatch",
                idx,
                screen_row
            );
        }
    }

    /// The sliced render must produce the same pixels as handing the whole
    /// history to Paragraph, at every scroll position.
    #[tokio::test]
    async fn test_sliced_render_matches_full_render() {
        let mut app = app_with_history(200);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        for scroll in [0usize, 1, 17, 100, 10_000] {
            app.output_scroll_position = scroll;
            terminal.draw(|f| render(f, &mut app)).unwrap();
            let sliced = terminal.backend().buffer().clone();

            let output_area = app.output_area.unwrap();
            let inner_width = output_area.width.saturating_sub(2) as usize;
            let inner_height = output_area.height.saturating_sub(2) as usize;

            // Same frame the old way: whole history handed to Paragraph.
            let total: usize = app
                .output_history
                .iter()
                .map(|l| crate::app::wrapped_rows(l, inner_width))
                .sum();
            let scroll_y = total
                .saturating_sub(inner_height)
                .saturating_sub(app.output_scroll_position);
            let mut reference = Terminal::new(TestBackend::new(100, 30)).unwrap();
            reference
                .draw(|f| {
                    let block = Block::default()
                        .title(" Output ")
                        .borders(Borders::ALL)
                        .border_style(Theme::muted());
                    let lines: Vec<Line> = app.output_history.iter().cloned().collect();
                    let paragraph = Paragraph::new(lines)
                        .block(block)
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_y as u16, 0));
                    f.render_widget(paragraph, output_area);
                })
                .unwrap();
            let full = reference.backend().buffer().clone();

            // Compare the inner output pane only (the reference frame has no
            // title/input panes, and the scrollbar overwrites the right border).
            for y in output_area.y + 1..output_area.y + output_area.height - 1 {
                for x in output_area.x + 1..output_area.x + output_area.width - 1 {
                    let pos = Position::new(x, y);
                    assert_eq!(
                        sliced.cell(pos).unwrap().symbol(),
                        full.cell(pos).unwrap().symbol(),
                        "scroll {} mismatch at ({}, {})",
                        scroll,
                        x,
                        y
                    );
                }
            }
        }
    }
}
