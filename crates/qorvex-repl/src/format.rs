//! Pretty formatters for output display.

use qorvex_core::element::UIElement;
use qorvex_core::memory::MemoryInfo;
use qorvex_core::simctl::SimulatorDevice;
use ratatui::text::{Line, Span};

use crate::ui::theme::Theme;

/// Format a UIElement as a styled Line.
///
/// Format: `[Type] id "label" @(x,y)`
pub fn format_element(elem: &UIElement) -> Line<'static> {
    let mut spans = Vec::new();

    // Element type
    let elem_type = elem.element_type.as_deref().unwrap_or("Unknown");
    spans.push(Span::styled(
        format!("[{}]", elem_type),
        Theme::element_type(),
    ));
    spans.push(Span::raw(" "));

    // Element ID
    if let Some(id) = &elem.identifier {
        spans.push(Span::styled(id.clone(), Theme::element_id()));
        spans.push(Span::raw(" "));
    }

    // Element label
    if let Some(label) = &elem.label {
        spans.push(Span::styled(
            format!("\"{}\"", label),
            Theme::element_label(),
        ));
        spans.push(Span::raw(" "));
    }

    // Element value
    if let Some(value) = &elem.value {
        spans.push(Span::styled(format!("={}", value), Theme::element_value()));
        spans.push(Span::raw(" "));
    }

    // Frame position
    if let Some(frame) = &elem.frame {
        spans.push(Span::styled(
            format!("@({:.0},{:.0})", frame.x, frame.y),
            Theme::muted(),
        ));
    }

    Line::from(spans)
}

/// Format a SimulatorDevice as a styled Line.
///
/// Format: `Name (State) UDID`
pub fn format_device(dev: &SimulatorDevice) -> Line<'static> {
    let state_style = if dev.state == "Booted" {
        Theme::device_booted()
    } else {
        Theme::device_shutdown()
    };

    Line::from(vec![
        Span::styled(dev.name.clone(), Theme::device_name()),
        Span::raw(" "),
        Span::styled(format!("({})", dev.state), state_style),
        Span::raw(" "),
        Span::styled(dev.udid.clone(), Theme::device_udid()),
    ])
}

/// Format a result status.
pub fn format_result(success: bool, message: &str) -> Line<'static> {
    if success {
        Line::from(vec![
            Span::styled("success", Theme::success()),
            if message.is_empty() {
                Span::raw("")
            } else {
                Span::styled(format!(": {}", message), Theme::muted())
            },
        ])
    } else {
        Line::from(vec![
            Span::styled("fail", Theme::error()),
            Span::styled(format!(": {}", message), Theme::muted()),
        ])
    }
}

/// Format a command input line for history display.
pub fn format_command(cmd: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("> ", Theme::prompt()),
        Span::raw(cmd.to_string()),
    ])
}

/// Format a [`MemoryInfo`] report as output lines.
///
/// Byte counts are shown in MiB as well as raw bytes, because a bare
/// nine-digit figure is unreadable at a glance.
pub fn format_memory_info(info: &MemoryInfo) -> Vec<Line<'static>> {
    let mib = |bytes: u64| format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    vec![
        Line::from("  App".to_string()),
        Line::from(format!("    PID:        {}", info.app.pid)),
        Line::from(format!(
            "    Footprint:  {} ({} bytes)",
            mib(info.app.footprint_bytes),
            info.app.footprint_bytes
        )),
        Line::from(format!("    Source:     {}", info.app.source)),
        Line::from("  Device".to_string()),
        Line::from(format!(
            "    Total:      {} ({} bytes)",
            mib(info.device.total_bytes),
            info.device.total_bytes
        )),
        Line::from(format!(
            "    Free:       {} ({} bytes)",
            mib(info.device.free_bytes),
            info.device.free_bytes
        )),
        Line::from(format!("    Pressure:   {:?}", info.device.pressure)),
        Line::from(format!("    Source:     {}", info.device.source)),
    ]
}
