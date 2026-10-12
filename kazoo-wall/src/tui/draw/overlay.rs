//! Popups over the wall: the key list, the module picker, the saves list,
//! and the questions asked before something goes or is replaced.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};

use crate::protocol::{KindInfo, KnobInfo, Place};

use super::super::app::{App, COMMAND_KEYS, CommandLine, GROUPS, SavesList, usage};
use super::super::log::{age, unix_seconds};
use super::super::theme::{
    AMBER, BRASS, INK, PANEL, PANEL_FOCUS, RED, TEXT, TEXT_DIM, bold, jack_glyph, plain,
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
        "add a module: ←→ or Tab / Shift-Tab change group, ↑↓ choose (what it is shows beside), \
         type to search every group, Ctrl-N names it first, < > make it smaller or bigger \
         (remembered); in the rack view it goes after the selected one",
    ),
    (
        ":",
        "the command line: add, remove, patch, unplug, turn, save, load, undo, keys, tag, untag, \
         group, groups (:tag kick osc1 klon2, then :group kick shows only them); Tab completes \
         kinds, modules, jacks, knobs, positions and saves, ↑↓ choose, ⏎ runs, Esc cancels",
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
        "switch between the list and the rack view (remembered, like c, f and the picker's size)",
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
    (
        "? / :keys",
        "this list (↑↓ scroll it, any other key closes it)",
    ),
    ("q / Ctrl-C", "leave the console; the wall keeps playing"),
    ("Q", "stop the wall for everyone (asks first)"),
    ("", "the mouse, in the rack view"),
    ("click", "select a module or a knob"),
    (
        "drag a knob",
        "up or down turns it (shift: finely); once clicked, the wheel turns it finely",
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
    (
        "scope",
        "a live trace on its screen (rack view): a green, b amber; under it trig'd, auto, \
         roll or xy, then the span and zoom",
    ),
    (
        "readout",
        "numbers for what is plugged in (rack view): a note, a CV, a gate's rate and duty, or \
         levels; a red ! when it gets non-numbers (its reset gate clears it)",
    ),
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

/// The module picker: a tab for each group across the top, the chosen
/// one (`tab`) lit; the kinds of that group, or with something typed
/// (`filter`) every kind that matches, grouped, the one at `index` chosen;
/// beside them (under them on a narrow screen) what the chosen kind is,
/// its jacks and its knobs; and, with a `place`, where on the rack the new
/// module goes.
pub fn picker(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    (tab, index, filter, place): (usize, usize, &str, Option<&Place>),
) {
    let kinds = app.picker_shown(tab, filter);
    let searching = !filter.trim().is_empty();
    let lit = Some(tab);
    let title = place.map_or_else(
        || "add a module".to_string(),
        |place| format!("add a module {}", app.place_words(place)),
    );
    let percent = u32::from(app.picker_percent());
    let scaled = |full: u16| u16::try_from(u32::from(full) * percent / 100).unwrap_or(full);
    let rect = popup(buf, area, scaled(area.width), scaled(area.height));
    let block = frame(&title, BRASS);
    let inner = block.inner(rect);
    block.render(rect, buf);
    if inner.height < 4 || inner.width < 10 {
        return;
    }
    let mut counts = [0_usize; GROUPS.len()];
    for (group, _) in app.picker_for(filter) {
        if let Some(count) = counts.get_mut(group) {
            *count += 1;
        }
    }
    Paragraph::new(tab_line(lit, &counts, usize::from(inner.width)))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buf);
    let find = if searching {
        Line::from(vec![
            Span::styled(" find in this category: ", plain(TEXT_DIM, PANEL)),
            Span::styled(format!("{filter}█"), bold(AMBER, PANEL)),
        ])
    } else {
        Line::from(Span::styled(
            " type to find a kind · ←→ or Tab change category█",
            plain(TEXT_DIM, PANEL),
        ))
    };
    Paragraph::new(find).render(Rect::new(inner.x, inner.y + 1, inner.width, 1), buf);
    Paragraph::new(Line::from(Span::styled(
        " ←→ Tab category · ↑↓ choose · ⏎ adds it · Ctrl-N names it first · Esc clears, then cancels",
        plain(TEXT_DIM, PANEL),
    )))
    .render(Rect::new(inner.x, inner.bottom() - 1, inner.width, 1), buf);
    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    let (list_area, detail_area) = if body.width >= 90 {
        let list_width = (body.width * 2 / 5).max(40);
        (
            Rect::new(body.x, body.y, list_width, body.height),
            Rect::new(
                body.x + list_width + 1,
                body.y,
                body.width - list_width - 2,
                body.height,
            ),
        )
    } else {
        let list_height = (body.height / 2).max(1);
        (
            Rect::new(body.x, body.y, body.width, list_height),
            Rect::new(
                body.x + 1,
                body.y + list_height,
                body.width.saturating_sub(2),
                body.height - list_height,
            ),
        )
    };
    picker_list(buf, list_area, &kinds, index, (filter, searching));
    if let Some((group, info)) = kinds.get(index) {
        Paragraph::new(kind_detail(info, *group))
            .wrap(Wrap { trim: false })
            .render(detail_area, buf);
    }
}

/// The picker's tabs: each group, with how many kinds it holds that match,
/// the `lit` one picked out; scrolled to keep it in `width` columns.
fn tab_line(lit: Option<usize>, counts: &[usize], width: usize) -> Line<'static> {
    let labels: Vec<String> = GROUPS
        .iter()
        .zip(counts)
        .map(|(group, count)| format!(" {group} {count} "))
        .collect();
    let widths: Vec<usize> = labels
        .iter()
        .map(|label| label.chars().count() + 1)
        .collect();
    // From the first tab that leaves the lit one in view.
    let lit_at = lit.unwrap_or(0).min(labels.len().saturating_sub(1));
    let mut first = 0;
    while first < lit_at && widths[first..=lit_at].iter().sum::<usize>() + 2 > width {
        first += 1;
    }
    let mut spans = vec![Span::styled(
        if first > 0 { "‹" } else { " " },
        plain(TEXT_DIM, PANEL),
    )];
    let mut used = 1;
    for (group, label) in labels.iter().enumerate().skip(first) {
        let wide = widths[group];
        if used + wide + 1 > width {
            spans.push(Span::styled("›", plain(TEXT_DIM, PANEL)));
            break;
        }
        used += wide;
        let style = if Some(group) == lit {
            bold(INK, BRASS)
        } else if counts.get(group).copied().unwrap_or(0) == 0 {
            plain(TEXT_DIM, PANEL)
        } else {
            bold(TEXT, PANEL_FOCUS)
        };
        spans.push(Span::styled(label.clone(), style));
        spans.push(Span::styled(" ", plain(TEXT, PANEL)));
    }
    Line::from(spans)
}

/// The picker's list of `kinds` (one category's) in `area`, the one at
/// `index` chosen and kept in view.
fn picker_list(
    buf: &mut Buffer,
    area: Rect,
    kinds: &[(usize, &KindInfo)],
    index: usize,
    (filter, searching): (&str, bool),
) {
    let width = usize::from(area.width);
    let mut rows: Vec<(Option<usize>, Line<'_>)> = Vec::new();
    for (position, (_, info)) in kinds.iter().enumerate() {
        let chosen = position == index;
        let background = if chosen { PANEL_FOCUS } else { PANEL };
        let lead = if chosen { "▸ " } else { "  " };
        let name = format!("{lead}{:<12}", truncate(&info.kind, 12));
        let about = truncate(&info.about, width.saturating_sub(16));
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
        let words = if searching {
            format!(" no kind of module matches '{filter}'")
        } else {
            " no kinds in this group: Tab for the next".to_string()
        };
        rows.push((None, Line::from(Span::styled(words, plain(AMBER, PANEL)))));
    }
    let visible = usize::from(area.height).max(1);
    let chosen_row = rows
        .iter()
        .position(|(position, _)| *position == Some(index))
        .unwrap_or(0);
    // Keep the chosen kind in view, with its heading when that fits.
    let first = (chosen_row + 2)
        .saturating_sub(visible)
        .min(chosen_row.saturating_sub(1));
    let lines: Vec<Line<'_>> = rows
        .into_iter()
        .skip(first)
        .take(visible)
        .map(|(_, line)| line)
        .collect();
    Paragraph::new(lines).render(area, buf);
}

/// What a kind is, for the picker: its name and group, what it does, its
/// inputs and outputs with what each carries, and its knobs with their
/// ranges.
fn kind_detail(info: &KindInfo, group: usize) -> Vec<Line<'static>> {
    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_uppercase(),
            bold(BRASS, PANEL).add_modifier(Modifier::UNDERLINED),
        ))
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(info.kind.clone(), bold(AMBER, PANEL)),
            Span::styled(
                format!(
                    " · {} · {}",
                    GROUPS.get(group).copied().unwrap_or("other"),
                    info.family
                ),
                plain(TEXT_DIM, PANEL),
            ),
        ]),
        Line::from(Span::styled(info.about.clone(), plain(TEXT, PANEL))),
    ];
    for (title, ports) in [("inputs", &info.inputs), ("outputs", &info.outputs)] {
        if ports.is_empty() {
            continue;
        }
        lines.push(heading(title));
        for port in ports {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} {:<10}", jack_glyph(port.signal, false), port.name),
                    bold(TEXT, PANEL),
                ),
                Span::styled(format!(" {}", port.about), plain(TEXT_DIM, PANEL)),
            ]));
        }
    }
    if !info.knobs.is_empty() {
        lines.push(heading("knobs · every knob is also a CV jack"));
        for knob in &info.knobs {
            lines.push(Line::from(vec![
                Span::styled(format!("◆ {:<10}", knob.name), bold(TEXT, PANEL)),
                Span::styled(format!(" {}", knob_words(knob)), plain(TEXT_DIM, PANEL)),
            ]));
        }
    }
    lines
}

/// A knob's travel in words: its named positions, or its range, with
/// where it starts.
fn knob_words(knob: &KnobInfo) -> String {
    let number = |value: f64| {
        let unit = if knob.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", knob.unit)
        };
        format!("{}{unit}", crate::format::number(value as f32))
    };
    let start = knob
        .labels
        .iter()
        .zip(0_u32..)
        .find(|(_, at)| (knob.min + f64::from(*at) - knob.default).abs() < 1e-6)
        .map_or_else(|| number(knob.default), |(label, _)| label.clone());
    if knob.labels.is_empty() {
        format!(
            "{} to {} · starts at {start}",
            number(knob.min),
            number(knob.max)
        )
    } else {
        let joiner = if knob.stepped { " · " } else { " → " };
        format!("{} · starts at {start}", knob.labels.join(joiner))
    }
}

/// Most completions shown at once over the command line.
const COMPLETIONS_SHOWN: usize = 10;

/// The command palette: a box floating near the top of the screen, as
/// in Iridium. Its first row is what is typed; under it why the line could
/// not run, then the completions of the word being typed (the chosen one
/// lit, each with what it is on the right); last, what the command takes.
pub fn command_line(buf: &mut Buffer, area: Rect, app: &App, line: &CommandLine) {
    let width = area.width.saturating_sub(4).min(96);
    if width < 20 || area.height < 6 {
        return;
    }
    let inner_width = usize::from(width - 2);
    let found = app.completions(line.typed());
    let chosen = line.chosen();
    let mut lines: Vec<Line<'_>> = vec![Line::from(vec![
        Span::styled(" > ", bold(BRASS, PANEL)),
        Span::styled(
            truncate(&format!(":{}█", line.text), inner_width.saturating_sub(3)),
            bold(AMBER, PANEL),
        ),
    ])];
    if let Some(error) = &line.error {
        lines.push(Line::from(Span::styled(
            truncate(&format!(" ✗ {error}"), inner_width),
            bold(RED, PANEL),
        )));
    }
    let shown = COMPLETIONS_SHOWN.min(usize::from(area.height.saturating_sub(8)).max(1));
    let first = chosen.map_or(0, |chosen| (chosen + 1).saturating_sub(shown));
    for (at, completion) in found.iter().enumerate().skip(first).take(shown) {
        let lit = Some(at) == chosen;
        let background = if lit { PANEL_FOCUS } else { PANEL };
        let word = truncate(&completion.word, inner_width.saturating_sub(4));
        let used = word.chars().count() + 3;
        let note = truncate(&completion.note, inner_width.saturating_sub(used + 3));
        let gap = inner_width.saturating_sub(used + note.chars().count() + 1);
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", if lit { "▸" } else { " " }),
                bold(BRASS, background),
            ),
            Span::styled(
                word,
                if lit {
                    bold(INK, BRASS)
                } else {
                    bold(TEXT, background)
                },
            ),
            Span::styled(" ".repeat(gap), plain(TEXT, background)),
            Span::styled(format!("{note} "), plain(TEXT_DIM, background)),
        ]));
    }
    if found.is_empty() && line.error.is_none() {
        lines.push(Line::from(Span::styled(
            "   nothing to complete here",
            plain(TEXT_DIM, PANEL),
        )));
    }
    if found.len() > shown {
        lines.push(Line::from(Span::styled(
            format!("   {} in all · Tab or ↑↓ for more", found.len()),
            plain(TEXT_DIM, PANEL),
        )));
    }
    let hint = usage(&line.text).unwrap_or_else(|| COMMAND_KEYS.to_string());
    lines.push(Line::from(Span::styled(
        truncate(&format!(" {hint}"), inner_width),
        plain(TEXT_DIM, PANEL),
    )));
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let rect = Rect::new(area.x + (area.width - width) / 2, area.y + 1, width, height);
    Clear.render(rect, buf);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(plain(BRASS, PANEL))
        .title(Span::styled(" command ", bold(BRASS, PANEL)))
        .style(plain(TEXT, PANEL));
    let inner = block.inner(rect);
    block.render(rect, buf);
    Paragraph::new(lines)
        .style(plain(TEXT, PANEL))
        .render(inner, buf);
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
    save: &crate::protocol::SaveInfo,
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
        crate::format::bpm(save.tempo),
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
