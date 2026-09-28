//! The wall: cream module panels on a dark rack, each with its knobs as
//! black dials and its jacks in the colours of their cables.

use std::collections::BTreeMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use kazoo_wall::protocol::{CableRecord, KnobView, ModuleView};

use super::super::app::{App, Focus, JackRef};
use super::super::knob::{Travel, show};
use super::super::layout::{self, JACK_LABEL, Placed};
use super::super::link::LinkState;
use super::super::theme::{
    AMBER, BRASS, CREAM, CREAM_FOCUS, INK, INK_DIM, JACK_EMPTY, KNOB, POINTER, RACK, RED, TEXT,
    TEXT_DIM, bold, cable_colour, jack_glyph, knob_pointer, plain,
};
use super::{fill, put, put_centred, put_right, text_width, truncate};

/// The colour a jack takes from its cables: the lowest-numbered cable in
/// it, and how many there are (outputs can fan out).
type JackCables = BTreeMap<String, (u32, usize)>;

fn jack_cables(cables: &[CableRecord]) -> (JackCables, JackCables) {
    let mut outputs = JackCables::new();
    let mut inputs = JackCables::new();
    for cable in cables {
        for (map, jack) in [(&mut outputs, &cable.from), (&mut inputs, &cable.to)] {
            map.entry(jack.clone())
                .and_modify(|(id, count)| {
                    *id = (*id).min(cable.id);
                    *count += 1;
                })
                .or_insert((cable.id, 1));
        }
    }
    (outputs, inputs)
}

/// The wall on its rack, scrolled to keep the selection in view. The
/// rack's top and bottom lines are rails, where the scroll hints go; the
/// panels hang between them.
pub fn wall(buf: &mut Buffer, rack: Rect, app: &mut App) {
    if rack.height < 3 || rack.width == 0 {
        return;
    }
    let area = Rect::new(rack.x, rack.y + 1, rack.width, rack.height - 2);
    let Some(snapshot) = app.snapshot() else {
        waiting(buf, area, app);
        return;
    };
    let mut area = area;
    if !app.link().is_up() {
        banner(buf, area, app);
        area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    }
    if snapshot.modules.is_empty() {
        let y = area.y + area.height / 2;
        put_centred(
            buf,
            area,
            y,
            "the wall is empty: press a to hang a module on it",
            plain(TEXT, RACK),
        );
        return;
    }
    let grid = layout::grid(area.width, &snapshot.modules);
    app.columns = grid.columns;
    app.scroll = match app.selected_index() {
        Some(index) => {
            let placed = grid.panels[index];
            layout::scroll_to(
                app.scroll,
                area.height,
                grid.height,
                &placed,
                placed.knob_line(app.knob_index()),
            )
        }
        None => app
            .scroll
            .min(grid.height.saturating_sub(u32::from(area.height))),
    };
    let app = &*app;
    let Some(snapshot) = app.snapshot() else {
        return;
    };
    let (outputs, inputs) = jack_cables(&snapshot.cables);
    let (hot, from) = app.patch_marks();
    let marks = Marks {
        hot: hot.as_ref(),
        from: from.as_ref(),
        outputs: &outputs,
        inputs: &inputs,
    };
    let selected = app.selected_index();
    let view = Sheet {
        area,
        scroll: app.scroll,
    };
    for (index, (module, placed)) in snapshot.modules.iter().zip(&grid.panels).enumerate() {
        let top = placed.y;
        let bottom = placed.y + u32::from(placed.height);
        if bottom <= view.scroll || top >= view.scroll + u32::from(area.height) {
            continue;
        }
        let focus = match (selected == Some(index), app.focus()) {
            (false, _) => PanelFocus::None,
            (true, Focus::Wall) => PanelFocus::Knob(app.knob_index()),
            (true, Focus::Cables | Focus::Log) => PanelFocus::Module,
        };
        let faulted = snapshot
            .faults
            .recent
            .iter()
            .any(|fault| fault.starts_with(&format!("{} ", module.id)));
        let context = Context {
            app,
            view: &view,
            marks: &marks,
        };
        panel(buf, &context, module, placed, focus, faulted);
    }
    if view.scroll > 0 {
        put_right(buf, rack, rack.y, " ▲ more above ", plain(TEXT_DIM, RACK));
    }
    if view.scroll + u32::from(area.height) < grid.height {
        put_right(
            buf,
            rack,
            rack.bottom() - 1,
            " ▼ more below ",
            plain(TEXT_DIM, RACK),
        );
    }
}

/// A window onto the wall's sheet.
struct Sheet {
    area: Rect,
    scroll: u32,
}

impl Sheet {
    /// The screen row of sheet line `line`, if it is in view.
    fn row(&self, line: u32) -> Option<u16> {
        let offset = line.checked_sub(self.scroll)?;
        let offset = u16::try_from(offset).unwrap_or(u16::MAX);
        (offset < self.area.height).then(|| self.area.y + offset)
    }
}

/// What every panel is drawn with.
struct Context<'a> {
    app: &'a App,
    view: &'a Sheet,
    marks: &'a Marks<'a>,
}

/// Where the patch cursor is, and which jacks have cables.
struct Marks<'a> {
    hot: Option<&'a JackRef>,
    from: Option<&'a JackRef>,
    outputs: &'a JackCables,
    inputs: &'a JackCables,
}

impl Marks<'_> {
    /// The style of jack `name` of `module`: the patch cursor, the
    /// cable's chosen output, a plugged jack in its cable's colour, or an
    /// empty one.
    fn jack(
        &self,
        module: &str,
        name: &str,
        output: bool,
        background: ratatui::style::Color,
    ) -> (bool, Style) {
        let is = |jack: Option<&JackRef>| {
            jack.is_some_and(|jack| {
                jack.module == module && jack.name == name && jack.output == output
            })
        };
        let map = if output { self.outputs } else { self.inputs };
        let cable = map.get(&format!("{module}.{name}"));
        let plugged = cable.is_some();
        let style = if is(self.hot) {
            bold(BRASS, INK).add_modifier(Modifier::REVERSED)
        } else if is(self.from) {
            bold(INK, BRASS)
        } else if let Some((id, _)) = cable {
            bold(cable_colour(*id), background)
        } else {
            plain(JACK_EMPTY, background)
        };
        (plugged, style)
    }
}

/// How a panel is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelFocus {
    /// Not selected.
    None,
    /// Selected, with the focus elsewhere (the cable list or the log).
    Module,
    /// Selected, with this knob chosen while the wall has the focus.
    Knob(usize),
}

/// One module's panel, and where it is drawn.
struct Panel<'a> {
    module: &'a ModuleView,
    focus: PanelFocus,
    faulted: bool,
    /// Its columns, clipped to the view.
    limit: Rect,
    edge: Style,
}

impl Panel<'_> {
    const fn x0(&self) -> u16 {
        self.limit.x
    }

    const fn width(&self) -> u16 {
        self.limit.width
    }

    const fn inner(&self) -> u16 {
        self.limit.width - 2
    }
}

/// Draw one module's panel.
fn panel(
    buf: &mut Buffer,
    context: &Context<'_>,
    module: &ModuleView,
    placed: &Placed,
    focus: PanelFocus,
    faulted: bool,
) {
    let view = context.view;
    let x0 = view.area.x + placed.x;
    let width = placed.width.min(view.area.right().saturating_sub(x0));
    if width < 4 {
        return;
    }
    let panel = Panel {
        module,
        focus,
        faulted,
        limit: Rect::new(x0, view.area.y, width, view.area.height),
        edge: if focus == PanelFocus::None {
            plain(INK_DIM, CREAM)
        } else {
            bold(BRASS, RACK)
        },
    };
    let mut line = placed.y;
    if let Some(y) = view.row(line) {
        top_edge(buf, &panel, y);
    }
    line += 1;

    let label_width = usize::from(panel.inner().saturating_sub(18).clamp(4, 10));
    for (index, knob) in module.knobs.iter().enumerate() {
        if let Some(y) = view.row(line) {
            let chosen = focus == PanelFocus::Knob(index);
            let background = if chosen { CREAM_FOCUS } else { CREAM };
            fill(buf, panel.limit, y, plain(INK, background));
            sides(buf, &panel, y);
            let row = Rect::new(x0 + 1, y, panel.inner(), 1);
            knob_row(buf, context, row, module, knob, chosen, label_width);
        }
        line += 1;
    }

    for (label, names, output) in [
        ("in", &module.inputs, false),
        ("out", &module.outputs, true),
    ] {
        for (row_index, row) in layout::jack_lines(names, panel.inner()).iter().enumerate() {
            if let Some(y) = view.row(line) {
                let lead = if row_index == 0 { label } else { "" };
                jack_row(buf, context, &panel, y, lead, row, output);
            }
            line += 1;
        }
    }

    if let Some(y) = view.row(line) {
        let rule = format!("╰{}╯", "─".repeat(usize::from(panel.inner())));
        put(buf, panel.limit, x0, y, &rule, panel.edge);
        let hands = context
            .app
            .snapshot()
            .and_then(|snapshot| snapshot.fingerprints.modules.get(&module.id))
            .map(hands_words)
            .unwrap_or_default();
        if !hands.is_empty() {
            let room = usize::from(panel.inner().saturating_sub(4));
            put(
                buf,
                panel.limit,
                x0 + 2,
                y,
                &truncate(&format!(" {hands} "), room),
                plain(INK_DIM, CREAM),
            );
        }
    }
}

/// Whose hands are on a module, most first: `Waffles 70% · Tom 30%`.
fn hands_words(shares: &BTreeMap<String, f64>) -> String {
    let mut seats: Vec<(&String, f64)> = shares
        .iter()
        .filter(|(_, share)| share.is_finite() && **share > 0.0)
        .map(|(seat, share)| (seat, *share))
        .collect();
    seats.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    seats
        .iter()
        .map(|(seat, share)| format!("{seat} {:.0}%", share * 100.0))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The top edge, with the module's id, name and kind engraved in it.
fn top_edge(buf: &mut Buffer, panel: &Panel<'_>, y: u16) {
    let (x0, width, inner) = (panel.x0(), panel.width(), panel.inner());
    let module = panel.module;
    fill(buf, panel.limit, y, plain(INK, CREAM));
    let rule = format!("╭{}╮", "─".repeat(usize::from(inner)));
    put(buf, panel.limit, x0, y, &rule, panel.edge);
    let (kind, kind_style) = if panel.faulted {
        (format!(" {} ⚠ ", module.kind), bold(RED, CREAM))
    } else {
        (format!(" {} ", module.kind), plain(INK_DIM, CREAM))
    };
    let kind_x = x0 + width - 1 - text_width(&kind).min(inner.saturating_sub(4));
    put(
        buf,
        Rect::new(x0, y, width - 1, 1),
        kind_x,
        y,
        &kind,
        kind_style,
    );
    let title_room = usize::from(kind_x.saturating_sub(x0 + 2));
    let title = module.name.as_ref().map_or_else(
        || format!(" {} ", module.id),
        |name| format!(" {} · {name} ", module.id),
    );
    let title_style = if panel.focus == PanelFocus::None {
        bold(INK, CREAM)
    } else {
        bold(INK, BRASS)
    };
    put(
        buf,
        panel.limit,
        x0 + 1,
        y,
        &truncate(&title, title_room),
        title_style,
    );
}

/// A line of jacks: inputs (`output` false) or outputs, `lead` labelling
/// the first line of each.
fn jack_row(
    buf: &mut Buffer,
    context: &Context<'_>,
    panel: &Panel<'_>,
    y: u16,
    lead: &str,
    jacks: &[usize],
    output: bool,
) {
    let Context { app, marks, .. } = *context;
    let module = panel.module;
    let names = if output {
        &module.outputs
    } else {
        &module.inputs
    };
    let face = plain(INK, CREAM);
    fill(buf, panel.limit, y, face);
    sides(buf, panel, y);
    let row = Rect::new(panel.x0() + 1, y, panel.inner(), 1);
    let mut x = put(
        buf,
        row,
        panel.x0() + 2,
        y,
        &format!("{lead:<width$}", width = usize::from(JACK_LABEL)),
        plain(INK_DIM, CREAM),
    );
    for &jack in jacks {
        let name = &names[jack];
        let signal = app.jack_signal(module, name, output);
        let (plugged, style) = marks.jack(&module.id, name, output, CREAM);
        x = put(
            buf,
            row,
            x,
            y,
            &jack_glyph(signal, plugged).to_string(),
            style,
        );
        let room = usize::from(row.right().saturating_sub(x));
        let hot = marks.hot.is_some_and(|hot| {
            hot.module == module.id && hot.name == *name && hot.output == output
        });
        let name_style = if hot { bold(INK, AMBER) } else { face };
        x = put(buf, row, x, y, &truncate(name, room), name_style);
        x = put(buf, row, x, y, " ", face);
    }
}

fn sides(buf: &mut Buffer, panel: &Panel<'_>, y: u16) {
    put(buf, panel.limit, panel.x0(), y, "│", panel.edge);
    put(
        buf,
        panel.limit,
        panel.x0() + panel.width() - 1,
        y,
        "│",
        panel.edge,
    );
}

/// One knob: its jack, its name, the dial and its value, with a glide in
/// progress shown as an arrow to where it is going.
fn knob_row(
    buf: &mut Buffer,
    context: &Context<'_>,
    row: Rect,
    module: &ModuleView,
    knob: &KnobView,
    chosen: bool,
    label_width: usize,
) {
    let Context { app, marks, .. } = *context;
    let background = if chosen { CREAM_FOCUS } else { CREAM };
    let y = row.y;
    let (plugged, jack_style) = marks.jack(&module.id, &knob.name, false, background);
    let mut x = put(
        buf,
        row,
        row.x + 1,
        y,
        &jack_glyph(kazoo_wall::catalogue::Signal::Cv, plugged).to_string(),
        jack_style,
    );
    let label_style = if chosen {
        bold(INK, background)
    } else {
        plain(INK, background)
    };
    x = put(
        buf,
        row,
        x + 1,
        y,
        &format!("{:<label_width$}", truncate(&knob.name, label_width)),
        label_style,
    );
    let info = app.knob_info(&module.kind, &knob.name);
    let travel = Travel::of(knob, info);
    let pointer = knob_pointer(travel.position(knob.value));
    let cap = if chosen {
        bold(BRASS, KNOB)
    } else {
        bold(POINTER, KNOB)
    };
    x = put(buf, row, x + 1, y, &format!(" {pointer} "), cap);
    x += 1;
    let room = usize::from(row.right().saturating_sub(x));
    let labels = info.map_or(&[][..], |info| info.labels.as_slice());
    let (text, style) = match app.ahead(&module.id, &knob.name) {
        Some(ahead) if !travel.same(ahead, knob.value) => (
            glide_text(
                &knob.display,
                &show(ahead, &knob.unit, labels, travel.min),
                room,
            ),
            bold(AMBER, background),
        ),
        _ if !travel.same(knob.value, knob.target) => (
            glide_text(&knob.display, &knob.target_display, room),
            bold(AMBER, background),
        ),
        _ => (truncate(&knob.display, room), plain(INK, background)),
    };
    put(buf, row, x, y, &text, style);
}

/// A value gliding to `target`: both when they fit, else just where it is
/// going.
fn glide_text(now: &str, target: &str, room: usize) -> String {
    let both = format!("{now} → {target}");
    if both.chars().count() <= room {
        both
    } else {
        truncate(&format!("→ {target}"), room)
    }
}

/// A line across the top of the wall while the daemon is not answering.
pub(super) fn banner(buf: &mut Buffer, area: Rect, app: &App) {
    let text = match app.link() {
        LinkState::Down {
            reason, attempts, ..
        } => format!(
            " ⚠ the wall is not answering ({reason}); reconnecting, attempt {attempts} · s starts it · this is the wall as last seen "
        ),
        LinkState::Connecting | LinkState::Connected { .. } => {
            " connecting to the wall… ".to_string()
        }
    };
    fill(buf, area, area.y, bold(TEXT, RED));
    put(
        buf,
        area,
        area.x,
        area.y,
        &truncate(&text, usize::from(area.width)),
        bold(TEXT, RED),
    );
}

/// The wall before its first snapshot.
pub(super) fn waiting(buf: &mut Buffer, area: Rect, app: &App) {
    let middle = area.y + area.height / 2;
    let width = usize::from(area.width);
    let lines: Vec<(String, Style)> = match app.link() {
        LinkState::Down {
            reason, attempts, ..
        } => vec![
            (
                format!("the wall is not answering at {}", app.socket()),
                bold(RED, RACK),
            ),
            (reason.clone(), plain(TEXT, RACK)),
            (
                format!("retrying (attempt {attempts}) · s starts it · q quits"),
                plain(TEXT_DIM, RACK),
            ),
        ],
        LinkState::Connecting | LinkState::Connected { .. } => {
            vec![("reaching for the wall…".to_string(), plain(TEXT, RACK))]
        }
    };
    for (row, (text, style)) in lines.iter().enumerate() {
        let y = middle.saturating_sub(1) + u16::try_from(row).unwrap_or(0);
        if y < area.bottom() {
            put_centred(buf, area, y, &truncate(text, width), *style);
        }
    }
}
