//! Completion popup widget.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::completion::{Candidate, CandidateKind, CompletionState};
use crate::ui::theme::Theme;

/// Widget for rendering the completion popup.
pub struct CompletionPopup<'a> {
    state: &'a CompletionState,
    /// Maximum number of candidates to show.
    max_visible: usize,
}

impl<'a> CompletionPopup<'a> {
    pub fn new(state: &'a CompletionState) -> Self {
        Self {
            state,
            max_visible: 8,
        }
    }

    /// Calculate the area for the popup based on cursor position.
    pub fn area(&self, cursor_x: u16, cursor_y: u16, container: Rect) -> Rect {
        let width = 50u16;
        let height = (self.state.candidates.len().min(self.max_visible) + 2) as u16;

        // Position above the input line if not enough space below
        let y = if cursor_y + height + 1 >= container.height {
            cursor_y.saturating_sub(height)
        } else {
            cursor_y + 1
        };

        // Ensure we don't overflow horizontally
        let x = cursor_x.min(container.width.saturating_sub(width));

        Rect::new(x, y, width.min(container.width), height.min(container.height))
    }
}

impl Widget for CompletionPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the background
        Clear.render(area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Theme::muted());

        let inner = block.inner(area);
        block.render(area, buf);

        // Determine visible range
        let total = self.state.candidates.len();
        let visible_count = self.max_visible.min(total);
        let selected = self.state.selected;

        // Calculate scroll offset to keep selection visible
        let scroll_offset = if selected >= visible_count {
            selected - visible_count + 1
        } else {
            0
        };

        for (i, candidate) in self.state.candidates
            .iter()
            .skip(scroll_offset)
            .take(visible_count)
            .enumerate()
        {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let is_selected = scroll_offset + i == selected;
            let line = format_candidate(candidate, is_selected, inner.width as usize);

            buf.set_line(inner.x, y, &line, inner.width);
        }
    }
}

fn format_candidate(candidate: &Candidate, selected: bool, max_width: usize) -> Line<'static> {
    let kind_indicator = match candidate.kind {
        CandidateKind::Command => "Cmd",
        CandidateKind::ElementId => "ID",
        CandidateKind::ElementLabel => "Lbl",
        CandidateKind::ElementSelectorById => "ID",
        CandidateKind::ElementSelectorByLabel => "Lbl",
        CandidateKind::DeviceUdid => "Dev",
        CandidateKind::BundleId => "App",
    };

    let base_style = if selected {
        Theme::selected()
    } else {
        Style::default()
    };

    let text_style = if selected {
        base_style
    } else {
        Theme::command()
    };

    let highlight_style = if selected {
        base_style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Theme::command().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    };

    let desc_style = if selected {
        base_style
    } else {
        Theme::description()
    };

    let kind_style = if selected {
        base_style.add_modifier(Modifier::DIM)
    } else {
        Theme::muted()
    };

    // Calculate available space for text and description
    let kind_len = kind_indicator.len() + 2; // " [Cmd]"
    let text_len = candidate.text.len();
    let separator_len = 2; // "  "

    let desc_max = max_width
        .saturating_sub(kind_len)
        .saturating_sub(text_len)
        .saturating_sub(separator_len);

    let desc = if candidate.description.len() > desc_max {
        format!("{}...", &candidate.description[..desc_max.saturating_sub(3)])
    } else {
        candidate.description.clone()
    };

    // Pad to fill width if selected
    let padding = if selected {
        let used = text_len + separator_len + desc.len() + kind_len;
        " ".repeat(max_width.saturating_sub(used))
    } else {
        String::new()
    };

    // Build text spans with match highlighting
    let text_spans = build_highlighted_spans(
        &candidate.text,
        &candidate.match_indices,
        text_style,
        highlight_style,
    );

    let mut spans = text_spans;
    spans.push(Span::styled("  ", base_style));
    spans.push(Span::styled(desc, desc_style));
    spans.push(Span::styled(padding, base_style));
    spans.push(Span::styled(format!(" [{}]", kind_indicator), kind_style));

    Line::from(spans)
}

/// Build spans with highlighted match indices.
fn build_highlighted_spans(
    text: &str,
    match_indices: &[usize],
    normal_style: Style,
    highlight_style: Style,
) -> Vec<Span<'static>> {
    if match_indices.is_empty() {
        return vec![Span::styled(text.to_string(), normal_style)];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut current_span = String::new();
    let mut in_highlight = false;

    for (i, ch) in chars.iter().enumerate() {
        let should_highlight = match_indices.contains(&i);

        if should_highlight != in_highlight {
            // Style change - flush current span
            if !current_span.is_empty() {
                let style = if in_highlight { highlight_style } else { normal_style };
                spans.push(Span::styled(current_span, style));
                current_span = String::new();
            }
            in_highlight = should_highlight;
        }

        current_span.push(*ch);
    }

    // Flush remaining
    if !current_span.is_empty() {
        let style = if in_highlight { highlight_style } else { normal_style };
        spans.push(Span::styled(current_span, style));
    }

    spans
}
