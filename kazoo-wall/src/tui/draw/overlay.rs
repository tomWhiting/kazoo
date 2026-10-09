//! Popups over the wall: the key list, the module picker, the saves list,
//! and the questions asked before something goes or is replaced.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};

use kazoo_wall::protocol::Place;

use super::super::app::{App, GROUPS, SavesList};
use super::super::log::{age, unix_seconds};
use super::super::theme::{
    AMBER, BRASS, INK, PANEL, PANEL_FOCUS, RED, TEXT, TEXT_DIM, bold, plain,
};
use super::truncate;

/// A popup `width` × `height` in the middle of `area`, cleared.
fn popup(buf: &mut Buffer, area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    Clear.render(rect, buf);
    rect
}

fn frame(title: &str, colour: ratatui::style::Color) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(plain(colour, PANEL))
        .title(Span::styled(format!(" {title} "), bold(colour, PANEL)))
        .style(plain(TEXT, PANEL))
}

/// Every key, grouped.
const HELP: &[(&str, &str)] = &[
    ("", "on the wall"),
    ("←→ / h l", "move between modules"),
    (
        "↑↓ / j k",
        "move between knobs (and on to the modules above and below)",
    ),
    ("= / -", "turn the knob up or down"),
    ("+ / _", "turn it finely (shift)"),
    ("PgUp PgDn, Alt", "turn it coarsely"),
    ("⏎", "type the knob's value (a number, or a named position)"),
    (
        "[ / ]",
        "shorter or longer glide for turns (at once to 64 beats)",
    ),
    (
        "a",
        "add a module: type to find a kind (Tab names it first); in the rack view it goes after the selected one",
    ),
    ("x", "remove the module and its cables (asks first)"),
    (
        "p",
        "patch: pick an output, then an input or knob, then the amount",
    ),
    (
        "u",
        "unplug the cable in the selected knob, or the cable chosen in the list",
    ),
    (
        "z",
        "undo the latest change not yet undone (in the log: the chosen one); \
         straight after a move, put the module back",
    ),
    ("t", "set the tempo (asks the desk when the wall is on it)"),
    (
        "g",
        "jump to a module: type its id, name or kind (again for the next)",
    ),
    ("", "the rack view"),
    (
        "Shift-arrows",
        "pan the rack a quarter of the screen that way",
    ),
    (".", "put the selected module in the middle of the screen"),
    (
        "H J K L",
        "move the selected module left, down, up or right (every console sees it)",
    ),
    (
        "c",
        "draw the rack full, compact (small dials) or as an overview",
    ),
    ("f", "put the log away for more rack, or bring it back"),
    ("", "anywhere"),
    (
        "v",
        "switch between the list and the rack view (kept until you quit)",
    ),
    (
        "Tab / Shift-Tab",
        "move between the wall, the cable list and the log",
    ),
    ("s", "start the wall, when it is not playing"),
    (
        "w",
        "save the wall as it is under a name (it plays on; asks before saving over a save)",
    ),
    (
        "o",
        "the saves: load one (⏎ then y; the wall as it is is kept first, and z undoes the \
         load), delete one (d then y), start an empty wall (n then y); a shows the wall's own",
    ),
    (
        "m",
        "hear the wall, or silence it again (it plays on, silent, by default)",
    ),
    (
        "r",
        "record what the wall plays to ~/Music/kazoo-wall, or stop (even while silent)",
    ),
    ("?", "this list (↑↓ scroll it, any other key closes it)"),
    ("q / Ctrl-C", "leave the console; the wall keeps playing"),
    ("Q", "stop the wall for everyone (asks first)"),
    ("", "the mouse, in the rack view"),
    ("click", "select a module or a knob"),
    (
        "drag a knob",
        "up or down turns it (shift: finely); the wheel turns it finely",
    ),
    ("double-click", "a knob goes back to its default"),
    (
        "drag a jack",
        "to another jack or onto a knob to patch (output to input)",
    ),
    (
        "right-click",
        "a cable, or the input or knob it is in, to unplug it",
    ),
    (
        "drag / wheel",
        "a panel or the bare rack pans it (shift-wheel pans sideways)",
    ),
    (
        "drag a title",
        "a faceplate's title bar moves it: the rack makes room; let go to hang it",
    ),
    (
        "middle-drag",
        "pans the rack from anywhere, over knobs and jacks too",
    ),
    (
        "blank panel",
        "click one (end of a row, or the spare row) or double-click bare rack to add a module there",
    ),
    (
        "click (overview)",
        "a module goes back to the faceplates, centred on it",
    ),
    (
        "cable to the edge",
        "carrying a cable to the rack's edge pans that way",
    ),
    ("◀ ▶ ▲ ▼", "the rack runs on that way"),
    ("", "on the panels"),
    ("( ↗ )", "a knob: the pointer shows where it is now"),
    ("→ 800 Hz", "a glide on its way to 800 Hz"),
    ("● ○", "audio jacks, filled when a cable is in"),
    ("■ □", "gate jacks"),
    ("◆ ◇", "CV jacks; every knob is one too"),
];

/// The key list.
/// The key list, scrolled `scroll` lines when it is taller than the
/// screen.
pub fn help(buf: &mut Buffer, area: Rect, scroll: usize) {
    let height = u16::try_from(HELP.len()).unwrap_or(u16::MAX - 2) + 2;
    let rect = popup(buf, area, 90, height);
    let visible = usize::from(rect.height.saturating_sub(2)).max(1);
    let scroll = scroll.min(HELP.len().saturating_sub(visible));
    let title = if visible < HELP.len() {
        "the wall's keys · ↑↓ for more"
    } else {
        "the wall's keys"
    };
    let key_width = 17;
    let lines: Vec<Line<'_>> = HELP
        .iter()
        .skip(scroll)
        .map(|(keys, what)| {
            if keys.is_empty() {
                Line::from(Span::styled(
                    format!(" {}", what.to_uppercase()),
                    bold(BRASS, PANEL).add_modifier(Modifier::UNDERLINED),
                ))
            } else {
                Line::from(vec![
                    Span::styled(format!(" {keys:<key_width$}"), bold(AMBER, PANEL)),
                    Span::styled((*what).to_string(), plain(TEXT, PANEL)),
                ])
            }
        })
        .collect();
    Paragraph::new(lines)
        .block(frame(title, BRASS))
        .render(rect, buf);
}

/// The module picker: every kind the catalogue has that matches what has
/// been typed (`filter`), grouped, the one at `index` chosen; and, with a
/// `place`, where on the rack the new module goes.
pub fn picker(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    (index, filter, place): (usize, &str, Option<&Place>),
) {
    let kinds = app.picker_for(filter);
    let mut rows: Vec<(Option<usize>, Line<'_>)> = Vec::new();
    let mut group = None;
    let inner_width = usize::from(area.width.saturating_sub(8).min(76));
    for (position, (kind_group, info)) in kinds.iter().enumerate() {
        if group != Some(*kind_group) {
            group = Some(*kind_group);
            let heading = GROUPS.get(*kind_group).copied().unwrap_or("other");
            rows.push((
                None,
                Line::from(Span::styled(
                    format!(" {}", heading.to_uppercase()),
                    bold(BRASS, PANEL),
                )),
            ));
        }
        let chosen = position == index;
        let background = if chosen { PANEL_FOCUS } else { PANEL };
        let lead = if chosen { "▸ " } else { "  " };
        let name = format!("{lead}{:<12}", truncate(&info.kind, 12));
        let about = truncate(&info.about, inner_width.saturating_sub(15));
        rows.push((
            Some(position),
            Line::from(vec![
                Span::styled(
                    name,
                    if chosen {
                        bold(INK, BRASS)
                    } else {
                        bold(TEXT, background)
                    },
                ),
                Span::styled(format!(" {about}"), plain(TEXT_DIM, background)),
            ]),
        ));
    }
    if rows.is_empty() {
        rows.push((
            None,
            Line::from(Span::styled(
                format!(" no kind of module matches '{filter}'"),
                plain(AMBER, PANEL),
            )),
        ));
    }
    let wanted = u16::try_from(rows.len()).unwrap_or(u16::MAX - 5) + 4;
    let rect = popup(buf, area, 80, wanted);
    let visible = usize::from(rect.height.saturating_sub(4)).max(1);
    let chosen_row = rows
        .iter()
        .position(|(position, _)| *position == Some(index))
        .unwrap_or(0);
    // Keep the chosen kind in view, with its heading when that fits.
    let first = (chosen_row + 2)
        .saturating_sub(visible)
        .min(chosen_row.saturating_sub(1));
    let find = if filter.is_empty() {
        Line::from(Span::styled(
            " type to find a kind█",
            plain(TEXT_DIM, PANEL),
        ))
    } else {
        Line::from(vec![
            Span::styled(" find: ", plain(TEXT_DIM, PANEL)),
            Span::styled(format!("{filter}█"), bold(AMBER, PANEL)),
        ])
    };
    let mut lines: Vec<Line<'_>> = vec![find];
    lines.extend(
        rows.into_iter()
            .skip(first)
            .take(visible)
            .map(|(_, line)| line),
    );
    lines.push(Line::from(Span::styled(
        " ↑↓ choose · ⏎ adds it · Tab names it first · Esc clears, then cancels",
        plain(TEXT_DIM, PANEL),
    )));
    let title = place.map_or_else(
        || "add a module".to_string(),
        |place| format!("add a module {}", app.place_words(place)),
    );
    Paragraph::new(lines)
        .block(frame(&title, BRASS))
        .render(rect, buf);
}

/// When a save was made, as Tom reads it: the local day and time, and how
/// long ago when the time now is known.
fn saved_when(at: &str, now: Option<i64>) -> String {
    let Some(seconds) = unix_seconds(at) else {
        return at.to_string();
    };
    let Some(local) = chrono::DateTime::from_timestamp(seconds, 0).map(|utc| {
        utc.with_timezone(&chrono::Local)
            .format("%a %-d %b %H:%M")
            .to_string()
    }) else {
        return at.to_string();
    };
    match now.map(|now| age(seconds, now)) {
        Some(ago) if ago == "now" => format!("{local} (just now)"),
        Some(ago) => format!("{local} ({ago} ago)"),
        None => local,
    }
}

/// The saves list: every save that can be loaded, newest first, the one
/// at `index` chosen, then the files that cannot be; with what the keys
/// do. `now` (Unix seconds) says how long ago each was saved.
pub fn saves(buf: &mut Buffer, area: Rect, app: &App, index: usize, now: Option<i64>) {
    let list = app.saves();
    let inner_width = usize::from(area.width.saturating_sub(8).min(92));
    let rows = save_rows(list, index, now, inner_width);
    let status = match (&list.problem, list.fetching) {
        (Some(problem), _) => format!(" the saves could not be read: {problem}"),
        (None, true) => " reading the saves…".to_string(),
        (None, false) => {
            " the wall plays on while a save loads: it fades out and back in".to_string()
        }
    };
    let wanted = u16::try_from(rows.len()).unwrap_or(u16::MAX - 6) + 5;
    let rect = popup(buf, area, 96, wanted);
    let visible = usize::from(rect.height.saturating_sub(5)).max(1);
    let chosen_row = rows
        .iter()
        .position(|(position, _)| *position == Some(index))
        .unwrap_or(0);
    // Keep the chosen save in view, with the heading when that fits.
    let first = (chosen_row + 2)
        .saturating_sub(visible)
        .min(chosen_row.saturating_sub(1));
    let mut lines: Vec<Line<'_>> = vec![Line::from(Span::styled(
        status,
        plain(
            if list.problem.is_some() {
                RED
            } else {
                TEXT_DIM
            },
            PANEL,
        ),
    ))];
    lines.extend(
        rows.into_iter()
            .skip(first)
            .take(visible)
            .map(|(_, line)| line),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ choose · ⏎ then y loads it · d then y deletes it · n empty wall · w saves · \
         a the wall's own · r reads again · Esc closes",
        plain(TEXT_DIM, PANEL),
    )));
    let count = list
        .result
        .as_ref()
        .map_or(0, |result| result.saves.len())
        .saturating_sub(list.own_count());
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(frame(&format!("the saves · {count}"), BRASS))
        .render(rect, buf);
}

/// One save's row in the list: its name, when it was saved, its tempo,
/// modules and cables; `chosen` under the cursor.
fn save_row(
    save: &kazoo_wall::protocol::SaveInfo,
    chosen: bool,
    now: Option<i64>,
    inner_width: usize,
) -> Line<'static> {
    let background = if chosen { PANEL_FOCUS } else { PANEL };
    let lead = if chosen { " ▸ " } else { "   " };
    let colour = if save.auto { TEXT_DIM } else { TEXT };
    let name = format!("{lead}{:<30}", truncate(&save.name, 30));
    let detail = format!(
        " {:<26} {:>8}  {} · {}",
        truncate(&saved_when(&save.saved_at, now), 26),
        kazoo_wall::format::bpm(save.tempo),
        save.modules,
        save.cables
    );
    Line::from(vec![
        Span::styled(
            name,
            if chosen {
                bold(INK, BRASS)
            } else {
                bold(colour, background)
            },
        ),
        Span::styled(
            truncate(&detail, inner_width.saturating_sub(33)),
            plain(colour, background),
        ),
    ])
}

/// The saves list's rows: a heading and a row for each save (with its
/// place among them), then the files that cannot be loaded.
fn save_rows(
    list: &SavesList,
    index: usize,
    now: Option<i64>,
    inner_width: usize,
) -> Vec<(Option<usize>, Line<'static>)> {
    let mut rows: Vec<(Option<usize>, Line<'static>)> = Vec::new();
    match &list.result {
        None => {}
        Some(result) if result.saves.is_empty() => rows.push((
            None,
            Line::from(Span::styled(
                " no saves yet: w saves the wall as it is",
                plain(TEXT_DIM, PANEL),
            )),
        )),
        Some(_) => {
            rows.push((
                None,
                Line::from(Span::styled(
                    format!(
                        "   {:<30} {:<26} {:>8}  {}",
                        "SAVE", "SAVED", "TEMPO", "MODULES · CABLES"
                    ),
                    bold(BRASS, PANEL),
                )),
            ));
            let shown = list.shown();
            if shown.is_empty() {
                rows.push((
                    None,
                    Line::from(Span::styled(
                        " none made by hand yet: w saves the wall as it is",
                        plain(TEXT_DIM, PANEL),
                    )),
                ));
            }
            let mut own_heading = false;
            for (position, save) in shown.into_iter().enumerate() {
                if save.auto && !own_heading {
                    own_heading = true;
                    rows.push((
                        None,
                        Line::from(Span::styled(
                            " THE WALL'S OWN: THE WALL AS IT WAS BEFORE EACH LOAD (a hides them)",
                            bold(BRASS, PANEL),
                        )),
                    ));
                }
                rows.push((
                    Some(position),
                    save_row(save, position == index, now, inner_width),
                ));
            }
            let own = list.own_count();
            if own > 0 && !list.show_auto {
                rows.push((
                    None,
                    Line::from(Span::styled(
                        format!(
                            " and {own} of the wall's own (the wall as it was before each load) \
                             · a shows them"
                        ),
                        plain(TEXT_DIM, PANEL),
                    )),
                ));
            }
        }
    }
    if let Some(result) = &list.result {
        if !result.broken.is_empty() {
            rows.push((
                None,
                Line::from(Span::styled(" CANNOT BE LOADED", bold(RED, PANEL))),
            ));
            for broken in &result.broken {
                rows.push((
                    None,
                    Line::from(Span::styled(
                        truncate(
                            &format!("   {} — {}", broken.name, broken.problem),
                            inner_width,
                        ),
                        plain(RED, PANEL),
                    )),
                ));
            }
        }
        if !result.pending.is_empty() {
            rows.push((
                None,
                Line::from(Span::styled(
                    format!(
                        " and {} more the wall is still reading · shown in a moment",
                        result.pending.len()
                    ),
                    plain(TEXT_DIM, PANEL),
                )),
            ));
        }
    }
    rows
}

/// Asking before a save replaces the wall.
pub fn confirm_load(buf: &mut Buffer, area: Rect, name: &str) {
    question(
        buf,
        area,
        "load a save",
        &[
            format!("Load \"{name}\" onto the wall?"),
            "The wall plays on: it fades out and back in.".to_string(),
            "The wall as it is now is saved first (before-load-…),".to_string(),
            "and z undoes the load.".to_string(),
        ],
        "y loads it · any other key goes back to the list",
    );
}

/// Asking before a save is deleted.
pub fn confirm_delete(buf: &mut Buffer, area: Rect, name: &str) {
    question(
        buf,
        area,
        "delete a save",
        &[
            format!("Delete the save \"{name}\" for good?"),
            "This cannot be undone; the wall itself is untouched.".to_string(),
        ],
        "y deletes it · any other key keeps it",
    );
}

/// Asking before the wall is cleared for an empty one.
pub fn confirm_clear(buf: &mut Buffer, area: Rect) {
    question(
        buf,
        area,
        "an empty wall",
        &[
            "Take everything off the wall for an empty one?".to_string(),
            "The wall as it is now is saved first (before-new-…),".to_string(),
            "and z undoes it.".to_string(),
        ],
        "y clears it · any other key goes back to the list",
    );
}

/// Asking before a save of that name is replaced.
pub fn confirm_overwrite(buf: &mut Buffer, area: Rect, name: &str) {
    question(
        buf,
        area,
        "save over a save",
        &[
            format!("A save named \"{name}\" is there already."),
            "Replace it with the wall as it is now?".to_string(),
            "The save as it was is gone for good.".to_string(),
        ],
        "y replaces it · any other key goes back to the name",
    );
}

/// Asking before a module goes.
pub fn confirm_remove(buf: &mut Buffer, area: Rect, module: &str, cables: usize) {
    let cables = match cables {
        0 => "It has no cables.".to_string(),
        1 => "Its cable goes with it.".to_string(),
        n => format!("Its {n} cables go with it."),
    };
    question(
        buf,
        area,
        "remove a module",
        &[
            format!("Take {module} off the wall?"),
            cables,
            "Anyone can undo it from the log.".to_string(),
        ],
        "y removes it · any other key keeps it",
    );
}

/// Asking before the wall stops.
pub fn confirm_stop(buf: &mut Buffer, area: Rect) {
    question(
        buf,
        area,
        "stop the wall",
        &[
            "Stop the wall for everyone?".to_string(),
            "It goes silent until someone starts it again;".to_string(),
            "the patch is saved and comes back as it was.".to_string(),
        ],
        "y stops it · any other key keeps it playing",
    );
}

fn question(buf: &mut Buffer, area: Rect, title: &str, lines: &[String], keys: &str) {
    let height = u16::try_from(lines.len()).unwrap_or(4) + 5;
    let rect = popup(buf, area, 56, height);
    let mut text: Vec<Line<'_>> = vec![Line::from("")];
    text.extend(
        lines
            .iter()
            .map(|line| Line::from(Span::styled(format!(" {line}"), plain(TEXT, PANEL)))),
    );
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        format!(" {keys}"),
        bold(AMBER, PANEL),
    )));
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(frame(title, RED))
        .render(rect, buf);
}
