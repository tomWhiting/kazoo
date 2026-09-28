//! The panes around the wall: the header, the cable list, the log and the
//! status line.

use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use kazoo_wall::MAX_CABLES;
use kazoo_wall::format;
use kazoo_wall::protocol::{CableRecord, ClockSource, Recording, Snapshot};

use super::super::app::{App, Focus, Mode, Patching, Tone, View, glide_words};
use super::super::knob::{Travel, show};
use super::super::link::LinkState;
use super::super::log::{Tone as LogTone, age};
use super::super::theme::{
    AMBER, BRASS, INK, METER_BG, PANEL, PANEL_FOCUS, RED, SAGE, STEEL, TEXT, TEXT_DIM, bold,
    cable_colour, plain,
};
use super::{fill, put, put_right, text_width, truncate};

/// Lowest level the meters show, in dBFS.
const METER_FLOOR: f64 = -60.0;

/// Horizontal eighth blocks for sub-cell meter resolution.
const EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// Beats in a bar, for the bar and beat readout.
const BEATS_PER_BAR: f64 = 4.0;

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// The header: clock, seats and link on the first line; the master meter
/// and the listen line on the second.
pub fn header(buf: &mut Buffer, area: Rect, app: &App) {
    let base = plain(TEXT, PANEL);
    for y in area.y..area.bottom() {
        fill(buf, area, y, base);
    }
    let y = area.y;
    let mut x = put(buf, area, area.x, y, " THE WALL ", bold(INK, BRASS));
    let (badge, colour) = link_badge(app);
    let badge_x = put_right(buf, area, y, &format!(" {badge} "), bold(colour, PANEL));
    let Some(snapshot) = app.snapshot() else {
        put(
            buf,
            area,
            x + 1,
            y,
            "waiting for the wall…",
            plain(TEXT_DIM, PANEL),
        );
        return;
    };
    let top = Rect::new(area.x, y, badge_x.saturating_sub(area.x), 1);
    x = put(buf, top, x + 1, y, "♩ ", plain(BRASS, PANEL));
    x = put(
        buf,
        top,
        x,
        y,
        &format::bpm(snapshot.tempo),
        bold(TEXT, PANEL),
    );
    x = separator(buf, top, x, y);
    x = put(
        buf,
        top,
        x,
        y,
        &bar_and_beat(snapshot.beat),
        plain(TEXT, PANEL),
    );
    x = separator(buf, top, x, y);
    let clock = match snapshot.clock {
        ClockSource::Desk => ("clock: desk", SAGE),
        ClockSource::Own => ("clock: own", TEXT),
    };
    x = put(buf, top, x, y, clock.0, plain(clock.1, PANEL));
    if snapshot.on_desk {
        x = put(buf, top, x, y, " (on the desk)", plain(SAGE, PANEL));
    }
    x = separator(buf, top, x, y);
    x = if snapshot.heard {
        put(buf, top, x, y, "heard", plain(SAGE, PANEL))
    } else {
        put(buf, top, x, y, "silent (m to hear)", plain(TEXT, PANEL))
    };
    if let Some(recording) = &snapshot.recording {
        x = separator(buf, top, x, y);
        x = put(buf, top, x, y, &rec_badge(recording), bold(INK, RED));
    }
    x = separator(buf, top, x, y);
    x = put(
        buf,
        top,
        x,
        y,
        &format!("glide {}", glide_words(app.glide_beats())),
        plain(TEXT, PANEL),
    );
    x = separator(buf, top, x, y);
    let seats = if snapshot.seats.is_empty() {
        "nobody".to_string()
    } else {
        snapshot.seats.join(", ")
    };
    let room = usize::from(top.right().saturating_sub(x + 8));
    x = put(buf, top, x, y, "seats: ", plain(TEXT_DIM, PANEL));
    put(buf, top, x, y, &truncate(&seats, room), bold(AMBER, PANEL));

    meters_and_listen(buf, area, area.y + 1, snapshot);
}

/// ` ● REC 3:20 `, and the samples lost when there are any.
fn rec_badge(recording: &Recording) -> String {
    if recording.dropped > 0 {
        format!(
            " ● REC {} · {} lost ",
            format::clock(recording.seconds),
            recording.dropped
        )
    } else {
        format!(" ● REC {} ", format::clock(recording.seconds))
    }
}

fn separator(buf: &mut Buffer, limit: Rect, x: u16, y: u16) -> u16 {
    put(buf, limit, x, y, "  │  ", plain(STEEL, PANEL))
}

/// `bar 12 · beat 3` for song position `beat`.
fn bar_and_beat(beat: f64) -> String {
    let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
    let bar = (beat / BEATS_PER_BAR).floor() as u64 + 1;
    let in_bar = (beat % BEATS_PER_BAR).floor() as u64 + 1;
    format!("bar {bar} · beat {in_bar}")
}

/// The link badge's words and colour.
fn link_badge(app: &App) -> (String, Color) {
    match app.link() {
        LinkState::Connected { .. } if app.feed().is_up() => ("● live".to_string(), SAGE),
        LinkState::Connected { .. } => ("● live feed down".to_string(), AMBER),
        LinkState::Connecting => ("○ connecting".to_string(), AMBER),
        LinkState::Down { .. } if app.launching() => ("○ starting".to_string(), AMBER),
        LinkState::Down { was_up: true, .. } => ("● lost: reconnecting".to_string(), RED),
        LinkState::Down { .. } => ("● not playing".to_string(), RED),
    }
}

fn meters_and_listen(buf: &mut Buffer, area: Rect, y: u16, snapshot: &Snapshot) {
    let mut x = put(buf, area, area.x + 1, y, "L ", plain(TEXT_DIM, PANEL));
    x = meter(buf, area, x, y, snapshot.levels.peak_l);
    x = put(buf, area, x + 2, y, "R ", plain(TEXT_DIM, PANEL));
    x = meter(buf, area, x, y, snapshot.levels.peak_r);
    x = separator(buf, area, x, y);
    let faults = snapshot.faults.count;
    let right = if faults > 0 {
        put_right(
            buf,
            area,
            y,
            &format!(" {faults} fault{} ", if faults == 1 { "" } else { "s" }),
            bold(RED, PANEL),
        )
    } else {
        area.right()
    };
    let room = usize::from(right.saturating_sub(x + 1));
    match &snapshot.listen {
        Some(listen) => {
            let words = format!("“{}”", listen.words);
            put(
                buf,
                area,
                x,
                y,
                &truncate(&words, room),
                plain(TEXT, PANEL).add_modifier(Modifier::ITALIC),
            );
        }
        None => {
            put(
                buf,
                area,
                x,
                y,
                &truncate("listening…", room),
                plain(TEXT_DIM, PANEL),
            );
        }
    }
}

/// A 16-cell peak meter for `db`, with its reading. Returns the column
/// after it.
fn meter(buf: &mut Buffer, area: Rect, x: u16, y: u16, db: f64) -> u16 {
    const CELLS: u16 = 16;
    let fraction = if db.is_finite() {
        ((db - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let eighths = (fraction * f64::from(CELLS) * 8.0).round() as u16;
    for cell in 0..CELLS {
        let filled = eighths.saturating_sub(cell * 8).min(8);
        let cell_db = METER_FLOOR * (1.0 - f64::from(cell + 1) / f64::from(CELLS));
        let colour = if cell_db > -3.0 {
            RED
        } else if cell_db > -12.0 {
            AMBER
        } else {
            SAGE
        };
        put(
            buf,
            area,
            x + cell,
            y,
            &EIGHTHS[usize::from(filled)].to_string(),
            plain(colour, METER_BG),
        );
    }
    let reading = if db.is_finite() && db > METER_FLOOR {
        format!(" {db:>5.1}")
    } else {
        "   -∞".to_string()
    };
    put(buf, area, x + CELLS, y, &reading, plain(TEXT, PANEL))
}

// ---------------------------------------------------------------------------
// Cables
// ---------------------------------------------------------------------------

/// The cable list: two lines per cable, in its colour.
pub fn cables(buf: &mut Buffer, area: Rect, app: &App) {
    let base = plain(TEXT, PANEL);
    for y in area.y..area.bottom() {
        fill(buf, area, y, base);
    }
    if area.height < 2 {
        return;
    }
    let focused = app.focus() == Focus::Cables;
    let title_style = if focused {
        bold(INK, BRASS)
    } else {
        bold(BRASS, PANEL)
    };
    let x = put(buf, area, area.x + 1, area.y, " CABLES ", title_style);
    let cables = app.cables();
    put(
        buf,
        area,
        x + 1,
        area.y,
        &format!("{} of {MAX_CABLES}", cables.len()),
        plain(TEXT_DIM, PANEL),
    );
    let list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    if cables.is_empty() {
        put(
            buf,
            list,
            list.x + 2,
            list.y + 1,
            &truncate("no cables yet: p patches one", usize::from(list.width) - 3),
            plain(TEXT_DIM, PANEL),
        );
        return;
    }
    let per_screen = usize::from(list.height / 2).max(1);
    let first = if focused {
        app.cable_cursor().saturating_sub(per_screen - 1)
    } else {
        0
    };
    let into_knob = selected_jack(app);
    for (row, (index, cable)) in cables
        .iter()
        .enumerate()
        .skip(first)
        .take(per_screen)
        .enumerate()
    {
        let y = list.y + u16::try_from(row * 2).unwrap_or(0);
        let chosen = focused && index == app.cable_cursor();
        let marked = into_knob.as_deref() == Some(cable.to.as_str());
        cable_entry(buf, list, y, cable, chosen, marked);
    }
    let below = cables.len().saturating_sub(first + per_screen);
    let hint = match (first, below) {
        (0, 0) => String::new(),
        (0, below) => format!(" ▼ {below} more "),
        (above, 0) => format!(" ▲ {above} more "),
        (above, below) => format!(" ▲ {above} ▼ {below} "),
    };
    if !hint.is_empty() {
        put_right(buf, area, area.y, &hint, plain(STEEL, PANEL));
    }
}

/// One cable: its colour, number, output and amount, then where it goes.
/// `chosen` is the list's cursor; `marked` is the cable in the knob
/// selected on the wall.
fn cable_entry(
    buf: &mut Buffer,
    list: Rect,
    y: u16,
    cable: &CableRecord,
    chosen: bool,
    marked: bool,
) {
    let background = if chosen { PANEL_FOCUS } else { PANEL };
    fill(buf, list, y, plain(TEXT, background));
    fill(buf, list, y + 1, plain(TEXT, background));
    let colour = cable_colour(cable.id);
    let lead = if chosen || marked { "▸" } else { " " };
    let mut cursor = put(buf, list, list.x, y, lead, bold(BRASS, background));
    cursor = put(buf, list, cursor, y, "━━ ", bold(colour, background));
    cursor = put(
        buf,
        list,
        cursor,
        y,
        &format!("#{} ", cable.id),
        plain(TEXT_DIM, background),
    );
    let amount = format!(" {:+.2} ", cable.amount);
    let amount_x = list.right() - text_width(&amount);
    let room = usize::from(amount_x.saturating_sub(cursor));
    put(
        buf,
        list,
        cursor,
        y,
        &truncate(&cable.from, room),
        bold(TEXT, background),
    );
    put(
        buf,
        list,
        amount_x,
        y,
        &amount,
        plain(amount_colour(cable.amount), background),
    );
    let cursor = put(
        buf,
        list,
        list.x + 1,
        y + 1,
        "  → ",
        bold(colour, background),
    );
    let room = usize::from(list.right().saturating_sub(cursor + 1));
    put(
        buf,
        list,
        cursor,
        y + 1,
        &truncate(&cable.to, room),
        plain(TEXT, background),
    );
}

const fn amount_colour(amount: f64) -> Color {
    if amount < 0.0 { AMBER } else { TEXT_DIM }
}

/// The jack of the knob selected on the wall, e.g. `vcf1.cutoff`.
fn selected_jack(app: &App) -> Option<String> {
    let index = app.selected_index()?;
    let module = app.snapshot()?.modules.get(index)?;
    let knob = module.knobs.get(app.knob_index())?;
    Some(format!("{}.{}", module.id, knob.name))
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

/// The live log, newest first.
pub fn log(buf: &mut Buffer, area: Rect, app: &App, unix_now: Option<i64>) {
    let base = plain(TEXT, PANEL);
    for y in area.y..area.bottom() {
        fill(buf, area, y, base);
    }
    let focused = app.focus() == Focus::Log;
    let rule = "─".repeat(usize::from(area.width));
    put(buf, area, area.x, area.y, &rule, plain(STEEL, PANEL));
    let title_style = if focused {
        bold(INK, BRASS)
    } else {
        bold(BRASS, PANEL)
    };
    let x = put(buf, area, area.x + 1, area.y, " LOG ", title_style);
    put(
        buf,
        area,
        x,
        area.y,
        " newest first · other seats in amber ",
        plain(TEXT_DIM, PANEL),
    );
    let lines = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let log = app.log();
    if log.is_empty() {
        put(
            buf,
            lines,
            lines.x + 2,
            lines.y,
            "nothing has happened yet",
            plain(TEXT_DIM, PANEL),
        );
        return;
    }
    let rows = usize::from(lines.height).max(1);
    let first = if focused {
        app.log_cursor().saturating_sub(rows - 1)
    } else {
        0
    };
    for (row, (index, line)) in log.lines().enumerate().skip(first).take(rows).enumerate() {
        let y = lines.y + u16::try_from(row).unwrap_or(0);
        let chosen = focused && index == app.log_cursor();
        let background = if chosen { PANEL_FOCUS } else { PANEL };
        fill(buf, lines, y, plain(TEXT, background));
        let when = match (line.at, unix_now) {
            (Some(at), Some(now)) => age(at, now),
            _ => String::new(),
        };
        let lead = if chosen { "▸" } else { " " };
        let mut cursor = put(buf, lines, lines.x, y, lead, bold(BRASS, background));
        cursor = put(
            buf,
            lines,
            cursor,
            y,
            &format!("{when:>4} "),
            plain(STEEL, background),
        );
        let number = line.seq.map_or_else(String::new, |seq| format!("#{seq}"));
        cursor = put(
            buf,
            lines,
            cursor,
            y,
            &format!("{number:>6} "),
            plain(TEXT_DIM, background),
        );
        let mut style = match line.tone {
            LogTone::Other => bold(AMBER, background),
            LogTone::Mine => plain(TEXT, background),
            LogTone::Seat => plain(SAGE, background),
            LogTone::Fault => bold(RED, background),
            LogTone::Note => plain(STEEL, background).add_modifier(Modifier::ITALIC),
        };
        let undone = line.seq.is_some_and(|seq| log.is_undone(seq));
        if undone {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        let room = usize::from(lines.right().saturating_sub(cursor + 1));
        let text = if undone {
            format!("{} (undone)", line.text)
        } else {
            line.text.clone()
        };
        put(buf, lines, cursor, y, &truncate(&text, room), style);
    }
}

// ---------------------------------------------------------------------------
// Status line
// ---------------------------------------------------------------------------

/// The status line: a prompt, the latest result, or the keys for where the
/// focus is.
pub fn status(buf: &mut Buffer, area: Rect, app: &App, now: Instant) {
    let base = plain(TEXT, PANEL);
    fill(buf, area, area.y, base);
    let width = usize::from(area.width.saturating_sub(2));
    if let Some((prompt, detail)) = prompt(app) {
        let x = put(
            buf,
            area,
            area.x + 1,
            area.y,
            &truncate(&prompt, width),
            bold(BRASS, PANEL),
        );
        let room = usize::from(area.right().saturating_sub(x + 1));
        put(
            buf,
            area,
            x,
            area.y,
            &truncate(&detail, room),
            plain(TEXT_DIM, PANEL),
        );
        return;
    }
    if let Some(status) = app.status(now) {
        let (mark, colour) = match status.tone {
            Tone::Done => ("✓ ", SAGE),
            Tone::Trouble => ("✗ ", RED),
            Tone::Info => ("· ", BRASS),
        };
        let x = put(buf, area, area.x + 1, area.y, mark, bold(colour, PANEL));
        put(
            buf,
            area,
            x,
            area.y,
            &truncate(&status.text, width.saturating_sub(2)),
            plain(colour, PANEL),
        );
        return;
    }
    let hints = match (app.focus(), app.view()) {
        (Focus::Wall, View::Rack) => {
            "mouse: drag knobs (shift fine, wheel fine), double-click resets, drag jack→jack \
             patches, right-click unplugs, drag the rack to pan · shift-arrows pan  . centre  \
             g jump  c density  f log  HJKL/drag a title to move · v list  ? help"
        }
        (Focus::Wall, View::List) => {
            "←→ module  ↑↓ knob  = - turn  + _ fine  PgUp/PgDn coarse  [ ] glide  ⏎ type  \
             p patch  u unplug  a add  x remove  z undo  t tempo  Tab panes  ? help  q quit"
        }
        (Focus::Cables, _) => "↑↓ cable  u unplug it  z undo  Tab panes  ? help  q quit",
        (Focus::Log, _) => "↑↓ change  z undo it  Tab panes  ? help  q quit",
    };
    put(
        buf,
        area,
        area.x + 1,
        area.y,
        &truncate(hints, width),
        plain(TEXT_DIM, PANEL),
    );
}

/// The prompt for a mode that takes typing or steps, and its help.
fn prompt(app: &App) -> Option<(String, String)> {
    match app.mode() {
        Mode::Tempo { text } => Some((
            format!("tempo: {text}█ BPM"),
            "   type 20 to 300 · ⏎ sets it · Esc cancels".to_string(),
        )),
        Mode::Value { module, knob, text } => {
            let range = app
                .snapshot()
                .and_then(|snapshot| snapshot.modules.iter().find(|view| view.id == *module))
                .and_then(|view| {
                    let knob_view = view.knobs.iter().find(|view| view.name == *knob)?;
                    let info = app.knob_info(&view.kind, knob);
                    let travel = Travel::of(knob_view, info);
                    let labels = info.map_or(&[][..], |info| info.labels.as_slice());
                    Some(format!(
                        "{} to {}",
                        show(travel.min, &knob_view.unit, labels, travel.min),
                        show(travel.max, &knob_view.unit, labels, travel.min)
                    ))
                })
                .unwrap_or_default();
            Some((
                format!("{module} {knob} = {text}█"),
                format!("   {range} · ⏎ turns it · Esc cancels"),
            ))
        }
        Mode::Name { kind, text, place } => Some((
            format!(
                "name the new {kind}{}: {text}█",
                place.as_ref().map_or_else(String::new, |place| format!(
                    " ({})",
                    app.place_words(place)
                ))
            ),
            "   A-Z a-z 0-9 space _ . - · ⏎ adds it (empty for no name) · Esc cancels".to_string(),
        )),
        Mode::Patch(Patching::From { module, port }) => {
            let jack = app
                .snapshot()
                .and_then(|snapshot| snapshot.modules.iter().find(|view| view.id == *module))
                .and_then(|view| view.outputs.get(*port))
                .map_or_else(String::new, |name| format!("{module}.{name}"));
            Some((
                format!("patch 1/3 · from {jack}"),
                "   ←→ module  ↑↓ output  ⏎ picks it  Esc cancels".to_string(),
            ))
        }
        Mode::Patch(Patching::To { from, module, jack }) => {
            let name = app
                .snapshot()
                .and_then(|snapshot| snapshot.modules.iter().find(|view| view.id == *module))
                .and_then(|view| {
                    super::super::app::jack_names(view)
                        .get(*jack)
                        .map(|name| (*name).to_string())
                })
                .map_or_else(String::new, |name| format!("{module}.{name}"));
            Some((
                format!("patch 2/3 · {from} → {name}"),
                "   ←→ module  ↑↓ input or knob  ⏎ picks it  Esc goes back".to_string(),
            ))
        }
        Mode::Patch(Patching::Amount {
            from,
            to,
            amount,
            replaces,
        }) => {
            let replacing =
                replaces.map_or_else(String::new, |cable| format!(" · replaces cable {cable}"));
            Some((
                format!("patch 3/3 · {from} → {to} · amount {amount:+.2}"),
                format!(
                    "{replacing}   = - by 0.05  + _ by 0.01  i inverts  ⏎ plugs in  Esc goes back"
                ),
            ))
        }
        Mode::Jump { text } => Some(jump_prompt(app, text)),
        Mode::Normal | Mode::Help { .. } | Mode::Add { .. } | Mode::Remove { .. } | Mode::Stop => {
            None
        }
    }
}

/// The jump prompt for `text`: what it would go to, and its help.
fn jump_prompt(app: &App, text: &str) -> (String, String) {
    let found = app
        .jump_target(text)
        .and_then(|index| app.snapshot()?.modules.get(index))
        .map_or_else(
            || {
                if text.trim().is_empty() {
                    String::new()
                } else {
                    " → nothing by that name".to_string()
                }
            },
            |module| {
                module.name.as_ref().map_or_else(
                    || format!(" → {}", module.id),
                    |name| format!(" → {} ({name})", module.id),
                )
            },
        );
    (
        format!("jump to: {text}█{found}"),
        "   a module's id, name or kind · ⏎ goes there · Esc cancels".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_beats_count_from_one() {
        assert_eq!(bar_and_beat(0.0), "bar 1 · beat 1");
        assert_eq!(bar_and_beat(5.5), "bar 2 · beat 2");
        assert_eq!(bar_and_beat(f64::NAN), "bar 1 · beat 1");
        assert_eq!(bar_and_beat(-3.0), "bar 1 · beat 1");
    }
}
