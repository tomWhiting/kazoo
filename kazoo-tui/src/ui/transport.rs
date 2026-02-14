//! Transport bar: play, record, stop, BPM, time display, beat indicator.
//!
//! Renders a single-row status bar showing the transport state indicator,
//! timeline position in both `MM:SS.mmm` and `Bar.Beat.Tick` formats,
//! BPM, loop/metronome toggles, visual beat dots, detected pitch, input
//! level, and CPU load.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, FocusedPanel};
use crate::theme;
use kazoo_core::transport::TransportState;

/// Draw the transport bar into the given area (expected to be 3 rows high).
#[allow(clippy::too_many_lines)]
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Transport ", FocusedPanel::Transport, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let transport = &app.display.transport;

    // State indicator with Unicode symbol.
    // During count-in, show "COUNT 2/4" instead of the normal state.
    let state_span = if transport.count_in_active {
        let count_str = format!(
            " COUNT {}/{}",
            transport.count_in_bar, transport.count_in_total
        );
        Span::styled(
            count_str,
            theme::style_recording(app.recording_blink_visible()),
        )
    } else {
        match transport.state {
            TransportState::Playing => Span::styled(" PLAY", theme::style_playing()),
            TransportState::Paused => Span::styled(" PAUSE", theme::style_paused()),
            TransportState::Stopped => Span::styled(" STOP", theme::style_stopped()),
            TransportState::Recording => {
                let visible = app.recording_blink_visible();
                Span::styled(" REC", theme::style_recording(visible))
            }
        }
    };

    // Time position formatted as MM:SS.mmm.
    let time_str = transport.position.format_time();

    // Bar.Beat.Tick position.
    let bar_beat = transport
        .position
        .format_bar_beat_tick(transport.bpm, transport.beats_per_bar);

    // Tempo display.
    let bpm_str = format!("{:.1} BPM", transport.bpm);

    // Loop indicator.
    let loop_str = if transport.loop_enabled { "LOOP" } else { "" };

    // Metronome indicator.
    let met_str = if transport.metronome_enabled {
        "MET"
    } else {
        ""
    };

    // Recording workflow indicator.
    let workflow_str = {
        use kazoo_core::transport::RecordingWorkflow;
        match app.recording_workflow {
            RecordingWorkflow::FreeRecord => String::new(),
            RecordingWorkflow::CountIn {
                count_in_bars,
                record_bars,
            } => {
                if record_bars == 0 {
                    format!("CI:{count_in_bars}")
                } else {
                    format!("CI:{count_in_bars}/{record_bars}b")
                }
            }
            RecordingWorkflow::FixedLength { bars } => format!("FIX:{bars}b"),
        }
    };

    // CPU load percentage.
    let cpu_str = format!("CPU: {:.0}%", app.display.cpu_load * 100.0);

    // Detected pitch.
    let pitch_str = app
        .display
        .pitch
        .frequency
        .map_or_else(|| String::from("--"), |f| format!("{f:.1}Hz"));

    // Input level.
    let input_db = app.display.input_level_db;
    let level_str = format!("In: {input_db:.0}dB");

    // Separator style.
    let sep = theme::style_text_dimmed();

    let mut spans = vec![
        Span::raw(" "),
        state_span,
        Span::styled(" | ", sep),
        Span::styled(time_str, theme::style_text()),
        Span::styled(" | ", sep),
        Span::styled(bar_beat, theme::style_text_secondary()),
        Span::styled(" | ", sep),
        Span::styled(bpm_str, theme::style_text()),
        Span::styled(" | ", sep),
        Span::styled(loop_str, theme::style_text()),
        Span::raw(" "),
        Span::styled(met_str, theme::style_text()),
    ];

    // Recording workflow indicator (visible when not FreeRecord).
    if !workflow_str.is_empty() {
        spans.push(Span::styled(" | ", sep));
        spans.push(Span::styled(workflow_str, theme::style_text_secondary()));
    }

    // Visual beat dots when metronome is enabled.
    if transport.metronome_enabled {
        spans.push(Span::raw(" "));
        for beat in 0..transport.beats_per_bar {
            let is_current = beat == transport.current_beat && transport.beat_active;
            let (symbol, style) = if is_current {
                ("\u{25cf}", Style::new().fg(theme::ACCENT_RECORD))
            } else {
                ("\u{25cb}", theme::style_text_dimmed())
            };
            spans.push(Span::styled(symbol, style));
            // Add space between dots for readability.
            if beat + 1 < transport.beats_per_bar {
                spans.push(Span::raw(" "));
            }
        }
    }

    spans.extend([
        Span::styled(" | ", sep),
        Span::styled(pitch_str, theme::style_text_secondary()),
        Span::styled(" | ", sep),
        Span::styled(level_str, theme::style_text_secondary()),
        Span::styled(" | ", sep),
        Span::styled(cpu_str, theme::style_text_dimmed()),
    ]);

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, inner);
}
