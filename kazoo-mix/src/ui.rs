//! Terminal rendering for the `kazoo-mix` desk.
//!
//! Drawn like an analogue console: vertical channel strips with a scribble
//! strip, clip light, trim, three-band EQ, aux send, pan, mute/solo and a
//! long-throw fader flanked by stereo peak meters; and a master section with
//! two moving-needle VU meters, the aux return, and a master fader with its
//! meter scale. Every control drawn registers a hit region so the mouse can
//! click, drag and scroll it.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine, Points};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::controls::{
    ChannelControls, FADER_OFF_DB, MasterControl, MasterControls, StripControl,
    fader_db_to_position,
};
use crate::desk::{Desk, Focus, Hit, HitTarget, StereoMeters, Throw};
use crate::engine::name_str;
use crate::meters::{
    METER_FLOOR_DBFS, PeakMeter, VU_MAX, VU_MIN, meter_fraction, vu_needle_fraction,
};
use crate::metronome::BEATS_PER_BAR;
use crate::shared::{ChannelReadout, DESK_CHANNELS, MasterReadout, SharedState};

/// Width of one channel strip including its border.
pub const STRIP_WIDTH: u16 = 12;

/// Width of the master section including its border.
pub const MASTER_WIDTH: u16 = 34;

/// Smallest terminal the desk will draw into.
pub const MIN_WIDTH: u16 = STRIP_WIDTH + MASTER_WIDTH;

/// Smallest terminal height the desk will draw into.
pub const MIN_HEIGHT: u16 = 18;

const BG: Color = Color::Rgb(0x17, 0x14, 0x12);
const PANEL: Color = Color::Rgb(0x24, 0x1E, 0x19);
const PANEL_FOCUS: Color = Color::Rgb(0x30, 0x28, 0x21);
const TEXT: Color = Color::Rgb(0xE7, 0xDC, 0xCB);
const TEXT_DIM: Color = Color::Rgb(0x96, 0x87, 0x74);
const BRASS: Color = Color::Rgb(0xCF, 0xA2, 0x47);
const SAGE: Color = Color::Rgb(0x87, 0xB3, 0x61);
const AMBER: Color = Color::Rgb(0xDE, 0xA2, 0x34);
const RED: Color = Color::Rgb(0xDE, 0x58, 0x45);
const STEEL: Color = Color::Rgb(0x73, 0x6B, 0x62);
const TAPE: Color = Color::Rgb(0xEF, 0xE6, 0xD2);
const INK: Color = Color::Rgb(0x2B, 0x22, 0x1A);
const METER_BG: Color = Color::Rgb(0x10, 0x0E, 0x0C);
const CAP: Color = Color::Rgb(0x55, 0x4A, 0x3F);
const LED_OFF: Color = Color::Rgb(0x4A, 0x3E, 0x34);
const VU_FACE: Color = Color::Rgb(0xE8, 0xD8, 0xA8);
const VU_RED: Color = Color::Rgb(0xB8, 0x32, 0x22);
const VU_BLUE: Color = Color::Rgb(0x2E, 0x4A, 0x7A);

/// Knob pointer from fully anticlockwise (7 o'clock) to fully clockwise
/// (5 o'clock).
const KNOB_POINTERS: [char; 7] = ['↙', '←', '↖', '↑', '↗', '→', '↘'];

/// Vertical eighth blocks for sub-cell meter resolution.
const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// dBFS marks printed beside the master meters.
const METER_MARKS: [f32; 8] = [0.0, -3.0, -6.0, -12.0, -20.0, -30.0, -40.0, -60.0];

/// dB marks printed beside the master fader.
const FADER_MARKS: [f32; 6] = [10.0, 0.0, -10.0, -20.0, -40.0, FADER_OFF_DB];

/// VU scale marks: (VU, major).
const VU_MARKS: [(f32, bool); 11] = [
    (-20.0, true),
    (-10.0, true),
    (-7.0, false),
    (-5.0, true),
    (-3.0, false),
    (-2.0, false),
    (-1.0, false),
    (0.0, true),
    (1.0, false),
    (2.0, false),
    (3.0, true),
];

/// Engine facts shown in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusInfo {
    /// Device sample rate.
    pub sample_rate: u32,
    /// Device output channels.
    pub channels: u16,
    /// Frames in the latest device callback, once audio is running.
    pub buffer_frames: Option<u32>,
    /// One line about what is feeding the desk: the latest hub news, or
    /// what to do next.
    pub note: String,
    /// Show the note as a warning.
    pub note_alert: bool,
    /// Audio stream errors since start.
    pub stream_errors: u64,
    /// Engine calls the audio callback made that were refused.
    pub engine_faults: u64,
    /// Whether the terminal accepted mouse capture; without it the desk is
    /// keyboard-only.
    pub mouse: bool,
}

/// Draw the whole desk and rebuild the hit map.
pub fn draw(frame: &mut Frame<'_>, desk: &mut Desk, shared: &SharedState, status: &StatusInfo) {
    draw_desk(frame, desk, shared, status);
    // The layout may have changed under a held fader (resize, bank change).
    desk.release_drag_if_moved();
}

fn draw_desk(frame: &mut Frame<'_>, desk: &mut Desk, shared: &SharedState, status: &StatusInfo) {
    desk.hits.clear();
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(BG).fg(TEXT));

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(frame.buffer_mut(), area);
        return;
    }

    let header = Rect::new(area.x, area.y, area.width, 1);
    let footer = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 2);

    let master_area = Rect::new(
        body.right() - MASTER_WIDTH,
        body.y,
        MASTER_WIDTH,
        body.height,
    );
    let strips_width = body.width - MASTER_WIDTH;
    let visible = usize::from(strips_width / STRIP_WIDTH).clamp(1, DESK_CHANNELS);
    desk.scroll_to_focus(visible);

    for index in 0..visible {
        let slot = desk.first_visible + index;
        let x = body.x + STRIP_WIDTH * u16::try_from(index).unwrap_or(0);
        let rect = Rect::new(x, body.y, STRIP_WIDTH, body.height);
        draw_strip(frame.buffer_mut(), rect, slot, desk, shared);
    }

    draw_master(frame, master_area, desk, shared);
    draw_header(frame.buffer_mut(), header, status, shared, &mut desk.hits);
    draw_footer(frame.buffer_mut(), footer, desk, shared, visible);

    if desk.help_open {
        draw_help(frame.buffer_mut(), area);
    }
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

/// Write `text` at `(x, y)`, clipped to `limit` so nothing spills outside the
/// region it belongs to. Returns the column after the last cell written.
fn put(buf: &mut Buffer, limit: Rect, x: u16, y: u16, text: &str, style: Style) -> u16 {
    if y < limit.y || y >= limit.bottom() || x >= limit.right() || x < limit.x {
        return x;
    }
    let width = usize::from(limit.right() - x);
    let (end, _) = buf.set_stringn(x, y, text, width, style);
    end
}

/// Write `text` centred within `width` columns starting at `x`.
fn put_centered(
    buf: &mut Buffer,
    limit: Rect,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    style: Style,
) {
    let len = u16::try_from(text.chars().count()).unwrap_or(width);
    let offset = width.saturating_sub(len) / 2;
    put(buf, limit, x + offset, y, text, style);
}

fn knob_glyph(normalized: f32) -> char {
    let normalized = if normalized.is_finite() {
        normalized.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let idx = (normalized * (KNOB_POINTERS.len() - 1) as f32).round() as usize;
    KNOB_POINTERS[idx.min(KNOB_POINTERS.len() - 1)]
}

fn format_db_short(db: f32) -> String {
    if db.abs() < 0.05 {
        "0.0".to_string()
    } else if db.abs() >= 10.0 {
        format!("{db:+.0}")
    } else {
        format!("{db:+.1}")
    }
}

fn format_fader_db(db: f32) -> String {
    if db <= FADER_OFF_DB {
        "-∞ dB".to_string()
    } else if db.abs() < 0.05 {
        "0.0 dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

fn format_level_db(db: f32) -> String {
    if db.is_finite() && db > METER_FLOOR_DBFS - 20.0 {
        format!("{db:.1}")
    } else {
        "-∞".to_string()
    }
}

fn format_amount(amount: f32) -> String {
    format!("{:.1}", amount * 10.0)
}

fn format_pan(pan: f32) -> String {
    let percent = (pan * 100.0).round();
    if percent.abs() < 1.0 {
        "C".to_string()
    } else if percent < 0.0 {
        format!("L{}", -percent as i32)
    } else {
        format!("R{}", percent as i32)
    }
}

fn strip_value_text(controls: &ChannelControls, control: StripControl) -> String {
    match control {
        StripControl::Trim => format_db_short(controls.trim_db),
        StripControl::EqHigh => format_db_short(controls.eq.high_db),
        StripControl::EqMid => format_db_short(controls.eq.mid_db),
        StripControl::EqLow => format_db_short(controls.eq.low_db),
        StripControl::Aux => format_amount(controls.aux_send),
        StripControl::Pan => format_pan(controls.pan.value()),
        StripControl::Fader => format_fader_db(controls.fader_db),
    }
}

fn zone_color(fraction: f32) -> Color {
    if fraction >= meter_fraction(-3.0) {
        RED
    } else if fraction >= meter_fraction(-12.0) {
        AMBER
    } else {
        SAGE
    }
}

/// What one knob row shows.
struct Knob<'a> {
    label: &'a str,
    /// Knob position, 0 to 1.
    normalized: f32,
    value: &'a str,
    focused: bool,
}

/// A knob row: engraved label, rotating pointer, value.
fn draw_knob_row(buf: &mut Buffer, limit: Rect, x: u16, y: u16, knob: &Knob<'_>) {
    let Knob {
        label,
        normalized,
        value,
        focused,
    } = *knob;
    let label_style = if focused {
        Style::new().fg(BG).bg(BRASS).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT_DIM).bg(PANEL)
    };
    let knob_style = if focused {
        Style::new()
            .fg(BRASS)
            .bg(PANEL_FOCUS)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(PANEL)
    };
    let value_style = if focused {
        Style::new().fg(TEXT).bg(PANEL_FOCUS)
    } else {
        Style::new().fg(TEXT_DIM).bg(PANEL)
    };
    let mut cursor = put(buf, limit, x, y, &format!("{label:<3}"), label_style);
    cursor = put(buf, limit, cursor, y, "(", knob_style);
    cursor = put(
        buf,
        limit,
        cursor,
        y,
        &knob_glyph(normalized).to_string(),
        knob_style,
    );
    cursor = put(buf, limit, cursor, y, ")", knob_style);
    put(buf, limit, cursor, y, &format!("{value:>4}"), value_style);
}

/// One vertical peak meter column with eighth-block resolution and a held
/// peak marker.
fn draw_meter_column(
    buf: &mut Buffer,
    limit: Rect,
    x: u16,
    width: u16,
    throw: Throw,
    meter: &PeakMeter,
) {
    let rows = usize::from(throw.rows);
    if rows == 0 {
        return;
    }
    let total_units = rows * 8;
    let level = meter_fraction(meter.level_db());
    let filled = (level * total_units as f32).round() as usize;
    let hold = meter_fraction(meter.hold_db());
    let hold_row = if hold > 0.0 {
        Some(((hold * rows as f32) as usize).min(rows - 1))
    } else {
        None
    };

    for r in 0..rows {
        let y = throw.top + u16::try_from(rows - 1 - r).unwrap_or(0);
        let zone = zone_color((r as f32 + 0.5) / rows as f32);
        let units = filled.saturating_sub(r * 8).min(8);
        let (glyph, style) = if units == 0 && hold_row == Some(r) {
            ('━', Style::new().fg(zone).bg(METER_BG))
        } else {
            (EIGHTHS[units], Style::new().fg(zone).bg(METER_BG))
        };
        let text: String = std::iter::repeat_n(glyph, usize::from(width)).collect();
        put(buf, limit, x, y, &text, style);
    }
}

/// A fader throw: track, unity mark and cap.
fn draw_fader(buf: &mut Buffer, limit: Rect, x: u16, throw: Throw, position: f32, focused: bool) {
    let unity_row = throw.row_at(fader_db_to_position(0.0));
    for offset in 0..throw.rows {
        let y = throw.top + offset;
        let (text, style) = if y == unity_row {
            ("─┼─", Style::new().fg(STEEL).bg(PANEL))
        } else {
            (" │ ", Style::new().fg(LED_OFF).bg(PANEL))
        };
        put(buf, limit, x, y, text, style);
    }
    let cap_style = if focused {
        Style::new().fg(BG).bg(BRASS).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(CAP).add_modifier(Modifier::BOLD)
    };
    put(buf, limit, x, throw.row_at(position), "━╋━", cap_style);
}

// ---------------------------------------------------------------------------
// Channel strip
// ---------------------------------------------------------------------------

/// Rows used above the fader throw inside a strip.
const STRIP_TOP_ROWS: u16 = 10;

/// What every part of a strip needs to draw itself.
#[derive(Debug, Clone, Copy)]
struct StripView {
    inner: Rect,
    slot: usize,
    controls: ChannelControls,
    readout: ChannelReadout,
    focused: Option<StripControl>,
    troubled: bool,
}

fn draw_strip(buf: &mut Buffer, rect: Rect, slot: usize, desk: &mut Desk, shared: &SharedState) {
    let controls = shared.channel_controls(slot);
    let readout = shared.channel_readout(slot);
    let focused = match desk.focus {
        Focus::Strip { slot: s, control } if s == slot => Some(control),
        _ => None,
    };

    let border = if focused.is_some() { BRASS } else { STEEL };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border).bg(PANEL))
        .title(Line::from(Span::styled(
            format!(" {:02} ", slot + 1),
            Style::new().fg(border).add_modifier(Modifier::BOLD),
        )))
        .style(Style::new().bg(PANEL));
    let inner = block.inner(rect);
    block.render(rect, buf);
    if inner.height < STRIP_TOP_ROWS + 3 || inner.width < 10 {
        return;
    }

    let view = StripView {
        inner,
        slot,
        controls,
        readout,
        focused,
        troubled: desk.strip_health[slot.min(DESK_CHANNELS - 1)].troubled(),
    };
    let meters = desk.strip_meters[slot.min(DESK_CHANNELS - 1)];
    draw_strip_head(buf, &view, &mut desk.hits);
    draw_strip_controls(buf, &view, &mut desk.hits);
    draw_strip_fader(buf, &view, &meters, &mut desk.hits);
}

/// Scribble strip, source status and clip light.
fn draw_strip_head(buf: &mut Buffer, view: &StripView, hits: &mut Vec<Hit>) {
    let inner = view.inner;
    let x = inner.x;
    let w = inner.width;
    let readout = &view.readout;

    // Scribble strip.
    let name = if readout.connected {
        name_str(&readout.name).to_string()
    } else {
        "—".to_string()
    };
    let tape = if readout.connected {
        Style::new().fg(INK).bg(TAPE).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT_DIM).bg(LED_OFF)
    };
    put(buf, inner, x, inner.y, &" ".repeat(usize::from(w)), tape);
    put_centered(buf, inner, x, inner.y, w, &name, tape);

    // Status and clip light.
    let status_y = inner.y + 1;
    let (status, status_style) = strip_status(readout, view.troubled);
    put(buf, inner, x, status_y, status, status_style);
    let (led, led_style) = if readout.clip {
        ("●", Style::new().fg(RED).add_modifier(Modifier::BOLD))
    } else {
        ("○", Style::new().fg(LED_OFF))
    };
    let led_x = x + w - 1;
    put(buf, inner, led_x, status_y, led, led_style.bg(PANEL));
    hits.push(Hit {
        area: Rect::new(led_x, status_y, 1, 1),
        target: HitTarget::ClipLights,
        throw: None,
    });
}

/// Trim, EQ, aux, pan and the mute/solo buttons.
fn draw_strip_controls(buf: &mut Buffer, view: &StripView, hits: &mut Vec<Hit>) {
    let inner = view.inner;
    let x = inner.x;
    let w = inner.width;
    let slot = view.slot;
    let controls = &view.controls;
    let focused_control = view.focused;

    // Knobs.
    let knob_rows = [
        (2, StripControl::Trim),
        (4, StripControl::EqHigh),
        (5, StripControl::EqMid),
        (6, StripControl::EqLow),
        (7, StripControl::Aux),
        (8, StripControl::Pan),
    ];
    put_centered(
        buf,
        inner,
        x,
        inner.y + 3,
        w,
        "·· EQ ··",
        Style::new().fg(STEEL).bg(PANEL),
    );
    for (row, control) in knob_rows {
        let y = inner.y + row;
        draw_knob_row(
            buf,
            inner,
            x,
            y,
            &Knob {
                label: control.label(),
                normalized: controls.normalized(control),
                value: &strip_value_text(controls, control),
                focused: focused_control == Some(control),
            },
        );
        hits.push(Hit {
            area: Rect::new(x, y, w, 1),
            target: HitTarget::Strip { slot, control },
            throw: None,
        });
    }

    // Mute / solo.
    let buttons_y = inner.y + 9;
    let mute_style = if controls.muted {
        Style::new().fg(BG).bg(RED).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT_DIM).bg(LED_OFF)
    };
    let solo_style = if controls.soloed {
        Style::new().fg(BG).bg(AMBER).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT_DIM).bg(LED_OFF)
    };
    let mute_x = x + 1;
    let solo_x = x + w - 4;
    put(buf, inner, mute_x, buttons_y, " M ", mute_style);
    put(buf, inner, solo_x, buttons_y, " S ", solo_style);
    hits.push(Hit {
        area: Rect::new(mute_x, buttons_y, 3, 1),
        target: HitTarget::Mute { slot },
        throw: None,
    });
    hits.push(Hit {
        area: Rect::new(solo_x, buttons_y, 3, 1),
        target: HitTarget::Solo { slot },
        throw: None,
    });
}

/// Fader throw with a peak meter either side, and the fader value.
fn draw_strip_fader(
    buf: &mut Buffer,
    view: &StripView,
    meters: &StereoMeters,
    hits: &mut Vec<Hit>,
) {
    let inner = view.inner;
    let x = inner.x;
    let w = inner.width;
    let slot = view.slot;
    let controls = &view.controls;
    let focused_control = view.focused;

    // Fader throw with meters either side.
    let throw = Throw {
        top: inner.y + STRIP_TOP_ROWS,
        rows: inner.height - STRIP_TOP_ROWS - 1,
    };
    draw_meter_column(buf, inner, x + 1, 2, throw, &meters.peak_left);
    draw_fader(
        buf,
        inner,
        x + 4,
        throw,
        controls.normalized(StripControl::Fader),
        focused_control == Some(StripControl::Fader),
    );
    draw_meter_column(buf, inner, x + 8, 2, throw, &meters.peak_right);
    hits.push(Hit {
        area: Rect::new(x, throw.top, w, throw.rows),
        target: HitTarget::Strip {
            slot,
            control: StripControl::Fader,
        },
        throw: Some(throw),
    });

    // Fader value.
    let value_style = if focused_control == Some(StripControl::Fader) {
        Style::new()
            .fg(BRASS)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(PANEL)
    };
    put_centered(
        buf,
        inner,
        x,
        inner.bottom() - 1,
        w,
        &format_fader_db(controls.fader_db),
        value_style,
    );
}

/// Source status badge: amber while the source has had trouble in the last
/// few seconds, green once it has been clean since.
const fn strip_status(readout: &ChannelReadout, troubled: bool) -> (&'static str, Style) {
    if readout.faulted {
        (
            "FAULT",
            Style::new().fg(RED).bg(PANEL).add_modifier(Modifier::BOLD),
        )
    } else if !readout.connected {
        ("open", Style::new().fg(TEXT_DIM).bg(PANEL))
    } else if troubled {
        (
            "LIVE",
            Style::new()
                .fg(AMBER)
                .bg(PANEL)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "LIVE",
            Style::new().fg(SAGE).bg(PANEL).add_modifier(Modifier::BOLD),
        )
    }
}

// ---------------------------------------------------------------------------
// Master section
// ---------------------------------------------------------------------------

/// What every part of the master section needs to draw itself.
#[derive(Debug, Clone, Copy)]
struct MasterView {
    inner: Rect,
    master: MasterControls,
    readout: MasterReadout,
    meters: StereoMeters,
    focused: Option<MasterControl>,
}

fn draw_master(frame: &mut Frame<'_>, rect: Rect, desk: &mut Desk, shared: &SharedState) {
    let focused = match desk.focus {
        Focus::Master { control } => Some(control),
        Focus::Strip { .. } => None,
    };
    let border = if focused.is_some() { BRASS } else { STEEL };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(border).bg(PANEL))
        .title(Line::from(Span::styled(
            " MASTER ",
            Style::new().fg(BRASS).add_modifier(Modifier::BOLD),
        )))
        .style(Style::new().bg(PANEL));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let view = MasterView {
        inner,
        master: shared.master_controls(),
        readout: shared.master_readout(),
        meters: desk.master_meters,
        focused,
    };
    let vu_rows = draw_master_faces(frame, inner, &view.meters);
    let buf = frame.buffer_mut();
    let return_y = inner.y + vu_rows;
    draw_master_return(buf, &view, return_y, &mut desk.hits);
    draw_master_fader(buf, &view, return_y + 1, &mut desk.hits);
}

/// The VU needle meters. Returns how many rows they used.
fn draw_master_faces(frame: &mut Frame<'_>, inner: Rect, meters: &StereoMeters) -> u16 {
    // Needle meters: two stacked if there is room, one shared face if not.
    let lower_rows: u16 = 1 + 3 + 1;
    let stacked_height = 6;
    let (vu_rows, stacked) = if inner.height >= stacked_height * 2 + lower_rows + 3 {
        (stacked_height * 2, true)
    } else {
        (
            inner
                .height
                .saturating_sub(lower_rows + 3)
                .clamp(3, stacked_height),
            false,
        )
    };
    if stacked {
        let top = Rect::new(inner.x, inner.y, inner.width, stacked_height);
        let bottom = Rect::new(
            inner.x,
            inner.y + stacked_height,
            inner.width,
            stacked_height,
        );
        draw_vu(frame, top, &[(meters.vu_left.needle(), INK, "L")]);
        draw_vu(frame, bottom, &[(meters.vu_right.needle(), INK, "R")]);
    } else {
        let face = Rect::new(inner.x, inner.y, inner.width, vu_rows);
        draw_vu(
            frame,
            face,
            &[
                (meters.vu_left.needle(), INK, "L"),
                (meters.vu_right.needle(), VU_BLUE, "R"),
            ],
        );
    }

    vu_rows
}

/// Aux-return knob and the master clip light.
fn draw_master_return(buf: &mut Buffer, view: &MasterView, return_y: u16, hits: &mut Vec<Hit>) {
    let inner = view.inner;
    let x = inner.x;
    let w = inner.width;
    let master = view.master;
    let focused_control = view.focused;

    // Return knob and clip light.
    draw_knob_row(
        buf,
        inner,
        x + 1,
        return_y,
        &Knob {
            label: MasterControl::AuxReturn.label(),
            normalized: master.normalized(MasterControl::AuxReturn),
            value: &format_amount(master.aux_return),
            focused: focused_control == Some(MasterControl::AuxReturn),
        },
    );
    put(
        buf,
        inner,
        x + 12,
        return_y,
        "verb",
        Style::new().fg(TEXT_DIM).bg(PANEL),
    );
    hits.push(Hit {
        area: Rect::new(x, return_y, 16, 1),
        target: HitTarget::Master {
            control: MasterControl::AuxReturn,
        },
        throw: None,
    });
    let (led, led_style) = if view.readout.clip {
        (
            "● CLIP",
            Style::new().fg(RED).bg(PANEL).add_modifier(Modifier::BOLD),
        )
    } else {
        ("○ CLIP", Style::new().fg(LED_OFF).bg(PANEL))
    };
    let led_x = x + w - 7;
    put(buf, inner, led_x, return_y, led, led_style);
    hits.push(Hit {
        area: Rect::new(led_x, return_y, 6, 1),
        target: HitTarget::ClipLights,
        throw: None,
    });
}

/// Master meters with their scale, the master fader with its scale, peak
/// readouts and the fader value.
fn draw_master_fader(buf: &mut Buffer, view: &MasterView, throw_top: u16, hits: &mut Vec<Hit>) {
    let inner = view.inner;
    let x = inner.x;
    let master = view.master;
    let meters = &view.meters;
    let focused_control = view.focused;

    // Meters, meter scale, fader, fader scale.
    let throw = Throw {
        top: throw_top,
        rows: inner.bottom().saturating_sub(throw_top + 1),
    };
    if throw.rows >= 3 {
        draw_meter_column(buf, inner, x + 1, 2, throw, &meters.peak_left);
        draw_meter_column(buf, inner, x + 4, 2, throw, &meters.peak_right);
        let mut labelled_rows = [u16::MAX; METER_MARKS.len()];
        for (index, mark) in METER_MARKS.into_iter().enumerate() {
            let row = throw.row_at(meter_fraction(mark));
            // On a short throw several marks share a row; keep the first
            // (loudest) rather than printing one over another.
            if labelled_rows.contains(&row) {
                continue;
            }
            labelled_rows[index] = row;
            let label = format!("{:>3}", mark as i32);
            put(
                buf,
                inner,
                x + 7,
                row,
                &label,
                Style::new().fg(STEEL).bg(PANEL),
            );
        }
        draw_fader(
            buf,
            inner,
            x + 13,
            throw,
            master.normalized(MasterControl::Fader),
            focused_control == Some(MasterControl::Fader),
        );
        let mut fader_rows = [u16::MAX; FADER_MARKS.len()];
        for (index, mark) in FADER_MARKS.into_iter().enumerate() {
            let row = throw.row_at(fader_db_to_position(mark));
            if fader_rows.contains(&row) {
                continue;
            }
            fader_rows[index] = row;
            let label = if mark <= FADER_OFF_DB {
                " -∞".to_string()
            } else {
                format!("{:>+3}", mark as i32).replace("+0", " 0")
            };
            put(
                buf,
                inner,
                x + 17,
                row,
                &label,
                Style::new().fg(STEEL).bg(PANEL),
            );
        }
        hits.push(Hit {
            area: Rect::new(x + 11, throw.top, 10, throw.rows),
            target: HitTarget::Master {
                control: MasterControl::Fader,
            },
            throw: Some(throw),
        });

        draw_master_readouts(buf, inner, x + 22, throw, meters);
    }

    // Master fader value.
    let value_style = if focused_control == Some(MasterControl::Fader) {
        Style::new()
            .fg(BRASS)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(PANEL)
    };
    let mut value = String::from("MST ");
    value.push_str(&format_fader_db(master.fader_db));
    put_centered(buf, inner, x, inner.bottom() - 1, 22, &value, value_style);
}

/// Peak and held-peak readouts beside the master fader.
fn draw_master_readouts(
    buf: &mut Buffer,
    inner: Rect,
    column: u16,
    throw: Throw,
    meters: &StereoMeters,
) {
    let peak_db = meters
        .peak_left
        .level_db()
        .max(meters.peak_right.level_db());
    let hold_db = meters.peak_left.hold_db().max(meters.peak_right.hold_db());
    put(
        buf,
        inner,
        column,
        throw.top,
        "peak",
        Style::new().fg(TEXT_DIM).bg(PANEL),
    );
    put(
        buf,
        inner,
        column,
        throw.top + 1,
        &format!("{:>6}", format_level_db(peak_db)),
        Style::new()
            .fg(zone_color(meter_fraction(peak_db)))
            .bg(PANEL),
    );
    if throw.rows >= 5 {
        put(
            buf,
            inner,
            column,
            throw.top + 3,
            "hold",
            Style::new().fg(TEXT_DIM).bg(PANEL),
        );
        put(
            buf,
            inner,
            column,
            throw.top + 4,
            &format!("{:>6}", format_level_db(hold_db)),
            Style::new().fg(TEXT).bg(PANEL),
        );
    }
}

/// Needle sweep: the needle rests at `VU_SWEEP_START` degrees (−20 VU) and
/// travels clockwise through `VU_SWEEP` degrees to +3 VU.
const VU_SWEEP_START: f64 = 140.0;
const VU_SWEEP: f64 = 100.0;

/// Needle pivot in canvas units; below the face, as on a real meter.
const VU_PIVOT: (f64, f64) = (50.0, -20.0);

/// Radius of the printed scale arc in canvas units.
const VU_RADIUS: f64 = 52.0;

/// Canvas coordinate space of a VU face.
const VU_WIDTH: f64 = 100.0;
const VU_HEIGHT: f64 = 50.0;

/// Needle angle in degrees for a needle fraction (0 = −20 VU, 1 = +3 VU).
fn needle_angle(fraction: f32) -> f64 {
    f64::from(fraction.clamp(-0.02, 1.05)).mul_add(-VU_SWEEP, VU_SWEEP_START)
}

fn arc_point(angle_degrees: f64, radius: f64) -> (f64, f64) {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    (
        radius.mul_add(cos, VU_PIVOT.0),
        radius.mul_add(sin, VU_PIVOT.1),
    )
}

/// A moving-coil VU meter face with one or more needles, each labelled in its
/// own colour in the corner of the face.
fn draw_vu(frame: &mut Frame<'_>, rect: Rect, needles: &[(f32, Color, &'static str)]) {
    if rect.width < 8 || rect.height < 3 {
        return;
    }
    let zero = vu_needle_fraction(0.0);
    // Canvas units per terminal column, to centre printed labels.
    let column_units = VU_WIDTH / f64::from(rect.width);
    let needles: Vec<(f32, Color, &'static str)> = needles.to_vec();
    let canvas = Canvas::default()
        .background_color(VU_FACE)
        .marker(Marker::Braille)
        .x_bounds([0.0, VU_WIDTH])
        .y_bounds([0.0, VU_HEIGHT])
        .paint(move |ctx: &mut Context<'_>| {
            let steps = 160;
            let mut black = Vec::with_capacity(steps);
            let mut red = Vec::with_capacity(steps / 3);
            for i in 0..=steps {
                let f = i as f32 / steps as f32;
                let point = arc_point(needle_angle(f), VU_RADIUS);
                if f >= zero {
                    red.push(point);
                } else {
                    black.push(point);
                }
            }
            ctx.draw(&Points {
                coords: &black,
                color: INK,
            });
            ctx.draw(&Points {
                coords: &red,
                color: VU_RED,
            });

            for (vu, major) in VU_MARKS {
                let angle = needle_angle(vu_needle_fraction(vu));
                let (x1, y1) = arc_point(angle, VU_RADIUS);
                let (x2, y2) = arc_point(angle, VU_RADIUS + if major { 5.0 } else { 2.5 });
                let color = if vu > 0.0 { VU_RED } else { INK };
                ctx.draw(&CanvasLine::new(x1, y1, x2, y2, color));
            }
            ctx.layer();

            for &(fraction, color, _) in &needles {
                let angle = needle_angle(fraction);
                let (x1, y1) = arc_point(angle, 21.0);
                let (x2, y2) = arc_point(angle, VU_RADIUS + 3.0);
                ctx.draw(&CanvasLine::new(x1, y1, x2, y2, color));
            }

            for (text, vu) in [("-20", VU_MIN), ("-7", -7.0), ("0", 0.0), ("+3", VU_MAX)] {
                let angle = needle_angle(vu_needle_fraction(vu));
                let (x, y) = arc_point(angle, VU_RADIUS + 11.0);
                let color = if vu > 0.0 { VU_RED } else { INK };
                let half_width = text.len() as f64 * column_units / 2.0;
                ctx.print(
                    (x - half_width).clamp(0.0, half_width.mul_add(-2.0, VU_WIDTH)),
                    y.min(VU_HEIGHT),
                    Span::styled(text.to_string(), Style::new().fg(color).bg(VU_FACE)),
                );
            }
            ctx.print(
                VU_WIDTH / 2.0 - column_units,
                VU_HEIGHT * 0.3,
                Span::styled(
                    "VU".to_string(),
                    Style::new()
                        .fg(INK)
                        .bg(VU_FACE)
                        .add_modifier(Modifier::BOLD),
                ),
            );
            for (index, &(_, color, label)) in needles.iter().enumerate() {
                ctx.print(
                    column_units * (index as f64).mul_add(2.0, 1.0),
                    VU_HEIGHT * 0.12,
                    Span::styled(
                        label.to_string(),
                        Style::new()
                            .fg(color)
                            .bg(VU_FACE)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
            }
        });
    frame.render_widget(canvas, rect);
}

// ---------------------------------------------------------------------------
// Header, footer, help, fallback
// ---------------------------------------------------------------------------

fn draw_header(
    buf: &mut Buffer,
    area: Rect,
    status: &StatusInfo,
    shared: &SharedState,
    hits: &mut Vec<Hit>,
) {
    buf.set_style(area, Style::new().bg(PANEL));
    let left_end = draw_device_info(buf, area, status);
    let left = Rect::new(area.x, area.y, left_end.saturating_sub(area.x), 1);

    let x = put(
        buf,
        left,
        area.x,
        area.y,
        " KAZOO MIX ",
        Style::new().fg(BG).bg(BRASS).add_modifier(Modifier::BOLD),
    );
    let x = draw_transport(buf, left, x, shared, hits);

    let note_style = if status.note_alert {
        Style::new()
            .fg(AMBER)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(PANEL)
    };
    put(buf, left, x + 2, area.y, &status.note, note_style);
}

/// Device facts and error counts at the right of the header, if there is
/// room. Returns where the left-hand content must stop.
fn draw_device_info(buf: &mut Buffer, area: Rect, status: &StatusInfo) -> u16 {
    let right = format!(
        "{}{:.1} kHz · {} ch · buf {} · ",
        if status.mouse { "" } else { "keys only · " },
        f64::from(status.sample_rate) / 1_000.0,
        status.channels,
        status
            .buffer_frames
            .map_or_else(|| "—".to_string(), |frames| frames.to_string()),
    );
    let plural = |count: u64, noun: &str| {
        if count == 1 {
            format!("1 {noun}")
        } else {
            format!("{count} {noun}s")
        }
    };
    let mut errors = plural(status.stream_errors, "stream error");
    if status.engine_faults > 0 {
        errors.push_str(" · ");
        errors.push_str(&plural(status.engine_faults, "engine fault"));
    }
    errors.push(' ');
    let errors_style = if status.stream_errors > 0 || status.engine_faults > 0 {
        Style::new().fg(RED).bg(PANEL).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT_DIM).bg(PANEL)
    };
    let total = u16::try_from(right.chars().count() + errors.chars().count()).unwrap_or(u16::MAX);
    if total + 60 < area.width {
        let start = area.right() - total;
        let after = put(
            buf,
            area,
            start,
            area.y,
            &right,
            Style::new().fg(TEXT_DIM).bg(PANEL),
        );
        put(buf, area, after, area.y, &errors, errors_style);
        start.saturating_sub(1)
    } else {
        area.right()
    }
}

/// The transport: play/stop button, tempo, beat lights and click state,
/// starting at `x`. Returns the column after it.
fn draw_transport(
    buf: &mut Buffer,
    left: Rect,
    mut x: u16,
    shared: &SharedState,
    hits: &mut Vec<Hit>,
) -> u16 {
    let y = left.y;
    let transport = shared.transport();
    let (button, button_style) = if transport.playing {
        (
            " ▶ PLAY ",
            Style::new().fg(BG).bg(SAGE).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            " ■ STOP ",
            Style::new()
                .fg(TEXT)
                .bg(PANEL_FOCUS)
                .add_modifier(Modifier::BOLD),
        )
    };
    let button_x = x + 1;
    x = put(buf, left, button_x, y, button, button_style);
    hits.push(Hit {
        area: Rect::new(button_x, y, x.saturating_sub(button_x), 1),
        target: HitTarget::PlayStop,
        throw: None,
    });
    x = put(
        buf,
        left,
        x + 1,
        y,
        &format!("♩ {:.1} BPM", transport.bpm),
        Style::new()
            .fg(BRASS)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD),
    );
    // One light per beat of the bar; the struck beat is lit.
    x += 1;
    let beat = shared.beat();
    for light in 0..BEATS_PER_BAR {
        let lit = beat == Some(light);
        let colour = match (lit, light) {
            (true, 0) => RED,
            (true, _) => AMBER,
            (false, _) => STEEL,
        };
        x = put(
            buf,
            left,
            x,
            y,
            if lit { "●" } else { "○" },
            Style::new().fg(colour).bg(PANEL),
        );
    }
    let (click, click_style) = if shared.click() {
        ("click on", Style::new().fg(TEXT).bg(PANEL))
    } else {
        ("click off", Style::new().fg(TEXT_DIM).bg(PANEL))
    };
    x = put(buf, left, x + 1, y, click, click_style);

    x
}

fn draw_footer(
    buf: &mut Buffer,
    area: Rect,
    desk: &mut Desk,
    shared: &SharedState,
    visible: usize,
) {
    buf.set_style(area, Style::new().bg(PANEL));
    let first = desk.first_visible + 1;
    let last = desk.first_visible + visible;
    let paged = visible < DESK_CHANNELS;
    let bank_text = |compact: bool| {
        paged.then(|| match (compact, first == last) {
            (false, _) => format!(" strips {first}–{last} of {DESK_CHANNELS} "),
            (true, true) => format!(" {first}/{DESK_CHANNELS} "),
            (true, false) => format!(" {first}–{last}/{DESK_CHANNELS} "),
        })
    };
    // The paging arrows and key hints always show; on a narrow desk they
    // shorten before the focus detail loses its source health.
    let layouts = [
        (bank_text(false), "  ? keys  q quit "),
        (bank_text(true), "  ? keys  q quit "),
        (bank_text(true), " ? q "),
    ];
    let widths = |bank: &Option<String>, keys: &str| {
        let bank_len = bank.as_ref().map_or(0, |text| {
            u16::try_from(text.chars().count()).unwrap_or(u16::MAX) + 2
        });
        let keys_len = u16::try_from(keys.chars().count()).unwrap_or(u16::MAX);
        (bank_len, keys_len)
    };
    let mut chosen = None;
    for (bank, keys) in &layouts {
        let (bank_len, keys_len) = widths(bank, keys);
        // One clear column always separates the detail from the hints.
        let Some(detail_width) = area
            .width
            .checked_sub(bank_len.saturating_add(keys_len).saturating_add(1))
        else {
            continue;
        };
        if let Some(detail) = footer_detail(desk, shared, detail_width) {
            chosen = Some((bank.clone(), *keys, detail));
            break;
        }
    }
    let (bank, keys, detail) = chosen.unwrap_or_else(|| {
        // Nothing fits whole: the tightest layout and the shortest detail,
        // cut off at the edge.
        let (bank, keys) = layouts[layouts.len() - 1].clone();
        (bank, keys, shortest_footer_detail(desk, shared))
    });
    let (bank_len, keys_len) = widths(&bank, keys);
    let controls_len = bank_len.saturating_add(keys_len);
    if controls_len > area.width {
        return;
    }
    let detail_width = (area.width - controls_len).saturating_sub(1);
    let detail_area = Rect::new(area.x, area.y, detail_width, 1);
    put(
        buf,
        detail_area,
        area.x,
        area.y,
        &detail,
        Style::new().fg(TEXT).bg(PANEL),
    );
    let mut x = area.right() - bank_len - keys_len;
    if let Some(bank) = bank {
        let arrow = Style::new()
            .fg(BRASS)
            .bg(PANEL_FOCUS)
            .add_modifier(Modifier::BOLD);
        put(buf, area, x, area.y, "◀", arrow);
        desk.hits.push(Hit {
            area: Rect::new(x, area.y, 1, 1),
            target: HitTarget::BankPrevious,
            throw: None,
        });
        x = put(
            buf,
            area,
            x + 1,
            area.y,
            &bank,
            Style::new().fg(TEXT).bg(PANEL),
        );
        put(buf, area, x, area.y, "▶", arrow);
        desk.hits.push(Hit {
            area: Rect::new(x, area.y, 1, 1),
            target: HitTarget::BankNext,
            throw: None,
        });
        x += 1;
    }
    put(
        buf,
        area,
        x,
        area.y,
        keys,
        Style::new().fg(TEXT_DIM).bg(PANEL),
    );
}

/// The footer's description of the focus, if some form of it fits in
/// `width` columns with the source's health counters intact.
fn footer_detail(desk: &Desk, shared: &SharedState, width: u16) -> Option<String> {
    let fits = |text: &String| text.chars().count() <= usize::from(width);
    match desk.focus {
        Focus::Strip { slot, control } => {
            strip_detail(shared, slot, control).into_iter().find(fits)
        }
        Focus::Master { control } => {
            let master = shared.master_controls();
            let value = match control {
                MasterControl::AuxReturn => format_amount(master.aux_return),
                MasterControl::Fader => format_fader_db(master.fader_db),
            };
            Some(format!(" MASTER · {} {value}", control.label())).filter(fits)
        }
    }
}

/// The shortest description of the focus, for a footer too narrow for any.
fn shortest_footer_detail(desk: &Desk, shared: &SharedState) -> String {
    match desk.focus {
        Focus::Strip { slot, control } => {
            let [.., shortest] = strip_detail(shared, slot, control);
            shortest
        }
        Focus::Master { .. } => footer_detail(desk, shared, u16::MAX).unwrap_or_default(),
    }
}

/// Descriptions of the focused strip, longest first. The source's health
/// counters (abbreviated u/s/r, explained in help) are in every form; the
/// control value and source name go first when space is short (the fader and
/// the strip head show them anyway).
fn strip_detail(shared: &SharedState, slot: usize, control: StripControl) -> [String; 4] {
    let readout = shared.channel_readout(slot);
    let controls = shared.channel_controls(slot);
    let channel = format!(" CH {:02}", slot + 1);
    let value = format!(
        "{} {}",
        control.label(),
        strip_value_text(&controls, control)
    );
    if readout.connected {
        let (u, sl, r) = (readout.underruns, readout.slips, readout.resyncs);
        let name = name_str(&readout.name);
        [
            format!("{channel} {name} · underruns {u} · slips {sl} · resyncs {r} · {value}"),
            format!(
                "{channel} {name} · {value} · u{} s{} r{}",
                compact_count(u),
                compact_count(sl),
                compact_count(r)
            ),
            format!(
                "{channel} {value} · u{} s{} r{}",
                compact_count(u),
                compact_count(sl),
                compact_count(r)
            ),
            format!(
                "{channel} u{} s{} r{}",
                compact_count(u),
                compact_count(sl),
                compact_count(r)
            ),
        ]
    } else {
        [
            format!("{channel} no source · {value}"),
            format!("{channel} {value} · no source"),
            format!("{channel} {value}"),
            format!("{channel} no source"),
        ]
    }
}

/// A counter in at most four characters: exact below 10 000, then in whole
/// thousands, millions and so on (`12k`, `3M`, up to `18E` for `u64::MAX`).
/// The shortest footer detail therefore always fits a minimum-width desk.
fn compact_count(count: u64) -> String {
    if count < 10_000 {
        return count.to_string();
    }
    let mut value = count;
    for unit in ['k', 'M', 'G', 'T', 'P', 'E'] {
        value /= 1_000;
        if value < 1_000 {
            return format!("{value}{unit}");
        }
    }
    // u64::MAX is about 18E, so the loop always returns.
    format!("{value}E")
}

/// Key help, derived from the desk size so it never disagrees with the keys.
fn help_lines() -> Vec<(String, String)> {
    let master_key = DESK_CHANNELS + 1;
    let strip_keys = if master_key <= 9 {
        format!("1 – {DESK_CHANNELS}, {master_key}")
    } else {
        format!("1 – {}", DESK_CHANNELS.min(9))
    };
    let strip_action = if master_key <= 9 {
        format!("jump to a strip, {master_key} = master")
    } else {
        "jump to a strip".to_string()
    };
    [
        (
            "← → / h l".to_string(),
            format!("move between strips (→ past {DESK_CHANNELS} reaches master)"),
        ),
        (
            "↑ ↓ / k j".to_string(),
            "move between controls on a strip".to_string(),
        ),
        (strip_keys, strip_action),
        ("+ / -".to_string(), "nudge the focused control".to_string()),
        ("} { / PgUp PgDn".to_string(), "big move".to_string()),
        ("0".to_string(), "reset the focused control".to_string()),
        (
            "R".to_string(),
            "reset the whole strip (or master)".to_string(),
        ),
        (
            "m / s".to_string(),
            "mute / solo the focused strip".to_string(),
        ),
        (
            "x / Backspace".to_string(),
            "clear all clip lights".to_string(),
        ),
        (
            "mouse click".to_string(),
            "focus a control; M / S toggle; ◀ ▶ page strips".to_string(),
        ),
        (
            "mouse drag".to_string(),
            "grab a fader cap and throw it".to_string(),
        ),
        (
            "mouse wheel".to_string(),
            "turn the knob or fader under the pointer".to_string(),
        ),
        (
            "?".to_string(),
            "this help (any key or click closes)".to_string(),
        ),
        (
            "space".to_string(),
            "play / stop the studio; every instrument follows".to_string(),
        ),
        ("t".to_string(), "tap tempo (tap on the beat)".to_string()),
        ("c".to_string(), "metronome click on / off".to_string()),
        ("[ ]".to_string(), "tempo down / up 1 BPM".to_string()),
        (
            "footer u s r".to_string(),
            "source underruns, slips, resyncs".to_string(),
        ),
        (
            "q / Esc / Ctrl-C".to_string(),
            "quit (Ctrl-Q and Ctrl-D too)".to_string(),
        ),
    ]
    .into()
}

fn draw_help(buf: &mut Buffer, area: Rect) {
    let width = 66.min(area.width.saturating_sub(4));
    let help = help_lines();
    let height = (u16::try_from(help.len()).unwrap_or(0) + 4).min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    Clear.render(popup, buf);
    let lines: Vec<Line<'_>> = help
        .iter()
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(
                    format!(" {keys:<17}"),
                    Style::new().fg(BRASS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(action.clone(), Style::new().fg(TEXT)),
            ])
        })
        .collect();
    Paragraph::new(lines)
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::new().fg(BRASS))
                .title(Span::styled(
                    " desk keys ",
                    Style::new().fg(BRASS).add_modifier(Modifier::BOLD),
                ))
                .style(Style::new().bg(PANEL_FOCUS)),
        )
        .render(popup, buf);
}

fn draw_too_small(buf: &mut Buffer, area: Rect) {
    let message = format!(
        "kazoo-mix needs {MIN_WIDTH}×{MIN_HEIGHT} — this terminal is {}×{}",
        area.width, area.height
    );
    let y = area.y + area.height / 2;
    put_centered(
        buf,
        area,
        area.x,
        y,
        area.width,
        &message,
        Style::new().fg(AMBER).bg(BG),
    );
    if area.height > 2 {
        put_centered(
            buf,
            area,
            area.x,
            y + 1,
            area.width,
            "q quits",
            Style::new().fg(TEXT_DIM).bg(BG),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Step;
    use crate::engine::{ChannelSnapshot, StereoLevel, short_name};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn status() -> StatusInfo {
        StatusInfo {
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: Some(256),
            note: "808 demo pattern".to_string(),
            note_alert: false,
            stream_errors: 0,
            engine_faults: 0,
            mouse: true,
        }
    }

    fn render(width: u16, height: u16, desk: &mut Desk, shared: &SharedState) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, desk, shared, &status()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn live_shared() -> SharedState {
        let shared = SharedState::new();
        shared.publish_channel(
            0,
            &ChannelSnapshot {
                connected: true,
                name: short_name("808"),
                peak: StereoLevel {
                    left: 0.7,
                    right: 0.7,
                },
                rms: StereoLevel {
                    left: 0.2,
                    right: 0.2,
                },
                clipped: true,
                ..ChannelSnapshot::EMPTY
            },
            256,
        );
        shared.publish_master(
            StereoLevel {
                left: 0.5,
                right: 0.4,
            },
            StereoLevel {
                left: 0.12,
                right: 0.1,
            },
            256,
            false,
        );
        shared
    }

    #[test]
    fn renders_at_many_sizes_without_panicking() {
        let shared = live_shared();
        for (width, height) in [
            (1, 1),
            (20, 5),
            (MIN_WIDTH - 1, MIN_HEIGHT),
            (MIN_WIDTH, MIN_HEIGHT),
            (80, 24),
            (100, 30),
            (130, 40),
            (220, 60),
            (400, 120),
        ] {
            let mut desk = Desk::new();
            desk.tick(&shared, 0.1);
            let desk_view = render(width, height, &mut desk, &shared);
            assert_eq!(desk_view.area, Rect::new(0, 0, width, height));
            desk.help_open = true;
            let help_view = render(width, height, &mut desk, &shared);
            assert_eq!(help_view.area, Rect::new(0, 0, width, height));
        }
    }

    #[test]
    fn full_desk_shows_every_strip_master_and_source() {
        let shared = live_shared();
        let mut desk = Desk::new();
        desk.tick(&shared, 0.1);
        let screen = text(&render(140, 34, &mut desk, &shared));
        assert!(screen.contains("KAZOO MIX"));
        assert!(screen.contains("808"));
        assert!(screen.contains("MASTER"));
        for slot in 1..=DESK_CHANNELS {
            assert!(
                screen.contains(&format!(" {slot:02} ")),
                "strip {slot} missing"
            );
        }
        assert!(screen.contains("VU"));
        assert!(
            !screen.contains("strips 1–"),
            "no bank indicator when all fit"
        );
    }

    #[test]
    fn narrow_terminal_banks_strips_and_follows_focus() {
        let shared = live_shared();
        shared.publish_channel(
            7,
            &ChannelSnapshot {
                connected: true,
                name: short_name("LONGNAMEHERE"),
                ..ChannelSnapshot::EMPTY
            },
            256,
        );
        let mut desk = Desk::new();
        desk.focus = Focus::Strip {
            slot: 7,
            control: StripControl::Fader,
        };
        let screen = text(&render(80, 22, &mut desk, &shared));
        assert!(screen.contains(" 08 "));
        assert!(!screen.contains(" 01 "));
        assert!(screen.contains("of 8"));
        assert!(
            desk.hits
                .iter()
                .any(|hit| hit.target == HitTarget::BankPrevious)
        );
        assert!(
            desk.hits
                .iter()
                .any(|hit| hit.target == HitTarget::BankNext)
        );
    }

    #[test]
    fn too_small_terminal_explains_itself() {
        let shared = SharedState::new();
        let mut desk = Desk::new();
        let screen = text(&render(40, 10, &mut desk, &shared));
        assert!(screen.contains("needs"));
        assert!(desk.hits.is_empty());
    }

    #[test]
    fn hit_map_covers_every_visible_control_inside_the_screen() {
        let shared = live_shared();
        let mut desk = Desk::new();
        let screen = Rect::new(0, 0, 140, 34);
        assert_eq!(render(140, 34, &mut desk, &shared).area, screen);
        for slot in 0..DESK_CHANNELS {
            for control in StripControl::ALL {
                assert!(
                    desk.hits
                        .iter()
                        .any(|hit| hit.target == HitTarget::Strip { slot, control }),
                    "missing {control:?} on strip {slot}"
                );
            }
            assert!(
                desk.hits
                    .iter()
                    .any(|hit| hit.target == HitTarget::Mute { slot })
            );
            assert!(
                desk.hits
                    .iter()
                    .any(|hit| hit.target == HitTarget::Solo { slot })
            );
        }
        for control in MasterControl::ALL {
            assert!(
                desk.hits
                    .iter()
                    .any(|hit| hit.target == HitTarget::Master { control })
            );
        }
        for hit in &desk.hits {
            assert_eq!(
                hit.area.intersection(screen),
                hit.area,
                "{hit:?} off screen"
            );
            if let Some(throw) = hit.throw {
                assert!(throw.rows >= 3);
                assert!(throw.top + throw.rows <= 34);
            }
        }
    }

    #[test]
    fn fader_cap_moves_with_the_control() {
        let shared = live_shared();
        let mut desk = Desk::new();
        let before = text(&render(140, 34, &mut desk, &shared));
        let mut controls = shared.channel_controls(0);
        controls.adjust(StripControl::Fader, -10, Step::Coarse);
        shared.store_channel_controls(0, controls);
        let after = text(&render(140, 34, &mut desk, &shared));
        assert_ne!(before, after);
        assert!(after.contains("-∞ dB") || after.contains("dB"));
    }

    #[test]
    fn value_formatting() {
        assert_eq!(format_db_short(0.0), "0.0");
        assert_eq!(format_db_short(3.5), "+3.5");
        assert_eq!(format_db_short(-12.0), "-12");
        assert_eq!(format_pan(0.0), "C");
        assert_eq!(format_pan(-0.5), "L50");
        assert_eq!(format_pan(1.0), "R100");
        assert_eq!(format_fader_db(FADER_OFF_DB), "-∞ dB");
        assert_eq!(format_fader_db(-2.25), "-2.2 dB");
        assert_eq!(format_amount(0.75), "7.5");
        assert_eq!(knob_glyph(0.0), '↙');
        assert_eq!(knob_glyph(0.5), '↑');
        assert_eq!(knob_glyph(1.0), '↘');
        assert_eq!(knob_glyph(f32::NAN), '↙');
        assert_eq!(format_level_db(f32::NEG_INFINITY), "-∞");
    }

    /// The resync count stays readable however narrow the desk is.
    #[test]
    fn footer_keeps_source_health_at_every_width() {
        let shared = live_shared();
        let mut desk = Desk::new();
        let full = text(&render(200, 30, &mut desk, &shared));
        let footer = full.lines().nth(29).unwrap();
        assert!(footer.contains("resyncs 0"), "{footer}");
        for width in [MIN_WIDTH, MIN_WIDTH + 20, 80, 100] {
            let screen = text(&render(width, 30, &mut desk, &shared));
            let footer = screen.lines().nth(29).unwrap();
            assert!(
                footer.contains("resyncs 0") || footer.contains("r0"),
                "{width}: {footer}"
            );
        }
        // Counters as large as they can get: the compact forms keep every
        // count, resyncs included, whole at the narrowest desk, with a clear
        // column before the bank arrows.
        shared.publish_channel(
            0,
            &ChannelSnapshot {
                connected: true,
                name: short_name("808"),
                underruns: u64::MAX,
                slips: 9_876_543_210,
                resyncs: u64::MAX,
                ..ChannelSnapshot::EMPTY
            },
            256,
        );
        let screen = text(&render(MIN_WIDTH, 30, &mut desk, &shared));
        let footer = screen.lines().nth(29).unwrap();
        assert!(footer.contains("u18E s9G r18E"), "{footer}");
        let before_arrow = footer.split('◀').next().unwrap();
        assert!(before_arrow.ends_with(' '), "{footer}");
        assert!(!footer.contains("808"), "{footer}");
    }

    #[test]
    fn compact_counts_never_exceed_four_characters() {
        for (count, text) in [
            (0, "0"),
            (9_999, "9999"),
            (10_000, "10k"),
            (999_999, "999k"),
            (1_000_000, "1M"),
            (9_876_543_210, "9G"),
            (u64::MAX, "18E"),
        ] {
            assert_eq!(compact_count(count), text);
        }
        let mut count = 1_u64;
        while let Some(next) = count.checked_mul(3) {
            assert!(compact_count(count).chars().count() <= 4, "{count}");
            count = next;
        }
    }
}
