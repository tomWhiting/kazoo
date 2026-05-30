//! TUI rendering for the 808 drum machine.

pub mod grid;
pub mod params;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use self::grid::GridWidget;
use self::params::ParamsWidget;
use crate::app::{App, Focus};
use crate::synth::VOICE_COUNT;

/// Static voice labels — avoids per-frame allocation.
const VOICE_LABELS: [&str; VOICE_COUNT] = [
    " KICK  ", " SNARE ", " CH    ", " OH    ", " CLAP  ", " TOM1  ", " TOM2  ", " TOM3  ",
    " COWB  ", " CYM   ",
];

/// Render the full 808 TUI.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Outer frame.
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .title(" 808 DRUM MACHINE ")
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .border_style(Style::new().fg(Color::DarkGray));
    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    // Layout: voice bar, grid with border, params with border, status bar.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Voice name bar.
            Constraint::Length(13), // Grid with border (2 border + 10 voices + 1 numbers).
            Constraint::Min(5),     // Params with border.
            Constraint::Length(1),  // Status bar.
        ])
        .split(inner);

    draw_voice_bar(frame, app, chunks[0]);
    draw_grid_panel(frame, app, chunks[1]);
    draw_params_panel(frame, app, chunks[2]);
    draw_status_bar(frame, app, chunks[3]);

    // Help overlay on top of everything.
    if app.show_help {
        draw_help_overlay(frame, area);
    }
}

/// Draw the voice name header bar (zero allocations).
fn draw_voice_bar(frame: &mut Frame, app: &App, area: Rect) {
    let buf = frame.buffer_mut();
    let y = area.y;
    if y >= area.y + area.height {
        return;
    }

    for (i, label) in VOICE_LABELS.iter().enumerate() {
        let style = if i == app.selected_voice {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::White)
        };
        for (j, ch) in label.chars().enumerate() {
            let x = area.x + (i as u16 * 7) + j as u16;
            if x < area.x + area.width {
                buf[(x, y)].set_char(ch).set_style(style);
            }
        }
    }
}

/// Draw the grid panel with a focus-aware border.
fn draw_grid_panel(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::Grid {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let title_style = if app.focus == Focus::Grid {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" GRID ")
        .title_style(title_style)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(GridWidget::new(app), inner);
}

/// Draw the params panel with a focus-aware border.
fn draw_params_panel(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::Params {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let title_style = if app.focus == Focus::Params {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" PARAMS ")
        .title_style(title_style)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(ParamsWidget::new(app), inner);
}

/// Draw the status bar at the bottom, adapting to terminal width.
#[allow(clippy::too_many_lines)]
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let w = area.width as usize;
    let pattern_name = &app.sequencer.current_pattern_ref().name;
    let pattern_count = app.sequencer.patterns.len();
    let bpm = app.sequencer.clock.bpm();
    let swing = app.sequencer.clock.swing();
    let playing_label = if app.sequencer.playing {
        "PLAY"
    } else {
        "STOP"
    };
    let playing_color = if app.sequencer.playing {
        Color::Green
    } else {
        Color::Red
    };
    let focus_label = match app.focus {
        Focus::Grid => "Grid",
        Focus::Params => "Params",
    };

    // Show pattern select mode indicator.
    let mode_indicator = if app.pattern_select_mode { " P+_" } else { "" };

    let mut spans = Vec::with_capacity(8);

    if w >= 80 {
        // Full layout.
        spans.push(Span::styled(
            format!(" Pat:{pattern_name}/{pattern_count}"),
            Style::new().fg(Color::Cyan),
        ));
        spans.push(Span::styled(
            format!("  Sw:{swing:.0}%"),
            Style::new().fg(Color::White),
        ));
        spans.push(Span::styled(
            format!("  BPM:{bpm:.0}"),
            Style::new().fg(Color::White),
        ));
        spans.push(Span::styled(
            format!("  [{playing_label}]"),
            Style::new().fg(playing_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("  {focus_label}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        if !mode_indicator.is_empty() {
            spans.push(Span::styled(mode_indicator, Style::new().fg(Color::Yellow)));
        }
        spans.push(Span::styled(
            "  [Space]=step [a]=acc [t]=trig [?]=help",
            Style::new().fg(Color::DarkGray),
        ));
    } else if w >= 50 {
        // Medium layout.
        spans.push(Span::styled(
            format!(" {pattern_name}"),
            Style::new().fg(Color::Cyan),
        ));
        spans.push(Span::styled(
            format!(" {bpm:.0}bpm"),
            Style::new().fg(Color::White),
        ));
        spans.push(Span::styled(
            format!(" [{playing_label}]"),
            Style::new().fg(playing_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {focus_label}"),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        if !mode_indicator.is_empty() {
            spans.push(Span::styled(mode_indicator, Style::new().fg(Color::Yellow)));
        }
        spans.push(Span::styled(" [?]=help", Style::new().fg(Color::DarkGray)));
    } else {
        // Minimal layout for narrow terminals.
        spans.push(Span::styled(
            format!(" {pattern_name}"),
            Style::new().fg(Color::Cyan),
        ));
        spans.push(Span::styled(
            format!(" {bpm:.0}"),
            Style::new().fg(Color::White),
        ));
        spans.push(Span::styled(
            format!(" [{playing_label}]"),
            Style::new().fg(playing_color).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {focus_label}"),
            Style::new().fg(Color::Yellow),
        ));
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line);
    frame.render_widget(para, area);
}

/// Draw the help overlay (centered popup with all keybindings).
fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    let popup_width = 52_u16.min(area.width.saturating_sub(4));
    let popup_height = 22_u16.min(area.height.saturating_sub(4));
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup.
    frame.render_widget(Clear, popup_area);

    let help_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " NAVIGATION",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("  Arrow keys    Move cursor / adjust params"),
        Line::from("  Tab           Switch Grid <-> Params focus"),
        Line::from("  1-9, 0        Select voice 1-10"),
        Line::from(""),
        Line::from(Span::styled(
            " SEQUENCER",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("  Space         Toggle step on/off"),
        Line::from("  a             Toggle accent (active steps)"),
        Line::from("  t             Trigger voice (audition)"),
        Line::from("  Enter         Play / Stop"),
        Line::from(""),
        Line::from(Span::styled(
            " TEMPO & PATTERNS",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("  +/-           Adjust BPM"),
        Line::from("  p + 1-9       Select pattern"),
        Line::from("  n             New pattern"),
        Line::from(""),
        Line::from(Span::styled(
            " GENERAL",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from("  q             Quit"),
        Line::from("  ?             This help screen"),
        Line::from(""),
        Line::from(Span::styled(
            "       Press any key to close",
            Style::new().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" HELP ")
        .title_style(Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .border_style(Style::new().fg(Color::Yellow));
    let para = Paragraph::new(help_lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, popup_area);
}
