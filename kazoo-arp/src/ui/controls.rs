//! Mode selector with Jupiter-8 style badge rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use kazoo_arp::ArpMode;

use crate::app::{App, Param};

/// Color constants (re-used from parent module).
const BLUE: Color = Color::Rgb(60, 140, 255);
const HOT: Color = Color::Rgb(255, 80, 50);
const SILVER: Color = Color::Rgb(170, 180, 200);
const DIM: Color = Color::Rgb(55, 60, 75);
const BORDER_DIM: Color = Color::Rgb(45, 48, 60);
const SEL_BG: Color = Color::Rgb(20, 35, 65);

/// Arrow/symbol prefix for each mode.
const fn mode_symbol(mode: ArpMode) -> &'static str {
    match mode {
        ArpMode::Up => "\u{25b2}",       // ▲
        ArpMode::Down => "\u{25bc}",     // ▼
        ArpMode::UpDown => "\u{2195}",   // ↕  (was ◆, now correct per spec)
        ArpMode::Random => "\u{2606}",   // ☆
        ArpMode::AsPlayed => "\u{25b8}", // ▸
    }
}

/// Render the mode selector row with Jupiter-8 style badges.
///
/// Each badge shows its shortcut key number (1-5).
pub fn draw_mode_selector(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(
            " MODE ",
            Style::new().fg(SILVER).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let is_mode_selected = app.selected_param == Param::Mode;
    let mut spans = Vec::with_capacity(ArpMode::ALL.len() * 3 + 1);
    spans.push(Span::styled(" ", Style::new().fg(DIM)));

    for (idx, &mode) in ArpMode::ALL.iter().enumerate() {
        let symbol = mode_symbol(mode);
        let key_num = idx + 1;
        let label = format!(" {key_num} {symbol} {} ", mode.label());

        let style = if mode == app.arp.mode {
            // Active mode: hot orange badge.
            Style::new()
                .fg(Color::Black)
                .bg(HOT)
                .add_modifier(Modifier::BOLD)
        } else if is_mode_selected {
            // Mode param is focused: show all modes in blue.
            Style::new().fg(BLUE).bg(SEL_BG)
        } else {
            // Unfocused, inactive mode.
            Style::new().fg(DIM)
        };

        spans.push(Span::styled(label, style));
        spans.push(Span::styled("  ", Style::new().fg(DIM)));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, inner);
}
