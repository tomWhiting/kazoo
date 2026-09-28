//! The rack view: every module a cream faceplate, its knobs round dials
//! (stepped knobs rotary switches with their detents), its jacks sockets,
//! and the cables hanging between them in their fingerprints' dye.

use std::collections::BTreeMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use kazoo_wall::catalogue::Signal;
use kazoo_wall::protocol::{KnobView, ModuleView, Snapshot};

use super::super::app::{App, Drag, Focus, PathKey};
use super::super::dye;
use super::super::knob::{Travel, show};
use super::super::rack::dial::{self, Part, braille};
use super::super::rack::geometry::{self, Blank, Density, Hanging, Hit, Plate, Point, Rack, Scale};
use super::super::theme::{
    AMBER, BRASS, CREAM, INK, INK_DIM, JACK_EMPTY, KNOB, METER_BG, RACK, RED, SAGE, STEEL,
    TEXT_DIM, bold, cable_colour, jack_glyph, plain,
};
use super::wall::{banner, waiting};
use super::{put_centred, truncate};

/// Lowest level the meters show, in dBFS.
const METER_FLOOR: f64 = -60.0;

/// Vertical eighth blocks for sub-cell meter resolution.
const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The window onto the rack's sheet.
#[derive(Debug, Clone, Copy)]
struct Window {
    area: Rect,
    pan: Point,
}

impl Window {
    /// The screen cell of sheet cell `point`, if it is in view.
    fn screen(&self, point: Point) -> Option<(u16, u16)> {
        let x = point.x.checked_sub(self.pan.x)?;
        let y = point.y.checked_sub(self.pan.y)?;
        let x = u16::try_from(x).unwrap_or(u16::MAX);
        let y = u16::try_from(y).unwrap_or(u16::MAX);
        (x < self.area.width && y < self.area.height).then(|| (self.area.x + x, self.area.y + y))
    }

    /// Write `text` from sheet cell `at`, a cell per character, skipping
    /// what is out of view.
    fn put(&self, buf: &mut Buffer, at: Point, text: &str, style: Style) {
        for (offset, ch) in text.chars().enumerate() {
            let point = Point {
                x: at.x + u32::try_from(offset).unwrap_or(u32::MAX / 2),
                y: at.y,
            };
            if let Some(cell) = self.screen(point) {
                buf[cell].set_char(ch).set_style(style);
            }
        }
    }

    /// Whether any of the box at `at`, `width` × `height`, is in view.
    fn shows(&self, at: Point, width: u32, height: u32) -> bool {
        at.x < self.pan.x + u32::from(self.area.width)
            && at.y < self.pan.y + u32::from(self.area.height)
            && at.x + width > self.pan.x
            && at.y + height > self.pan.y
    }

    /// Write `text` centred in `width` cells from `at`.
    fn centred(&self, buf: &mut Buffer, at: Point, width: u16, text: &str, style: Style) {
        let text = truncate(text, usize::from(width));
        let used = u32::try_from(text.chars().count()).unwrap_or(0);
        let offset = (u32::from(width).saturating_sub(used)) / 2;
        self.put(
            buf,
            Point {
                x: at.x + offset,
                y: at.y,
            },
            &text,
            style,
        );
    }
}

/// What every faceplate is drawn with.
#[derive(Clone, Copy)]
struct Scene<'a> {
    window: &'a Window,
    app: &'a App,
    snapshot: &'a Snapshot,
    /// The colour of each cable, and of the cable in each jack.
    plugs: &'a Plugs<'a>,
    /// What the carried cable is over, if the mouse is carrying one.
    over: Option<&'a Hit>,
}

/// The colour of cable `id`: its fingerprint's dye, or (with no dye yet)
/// its own colour by number.
fn cable_hue(snapshot: &Snapshot, id: u32) -> Color {
    snapshot
        .fingerprints
        .cables
        .get(&id.to_string())
        .and_then(dye::mix)
        .unwrap_or_else(|| cable_colour(id))
}

/// Every cable's colour, and the colour of the lowest-numbered cable in
/// each jack, worked out once a frame.
#[derive(Default)]
struct Plugs<'a> {
    hues: BTreeMap<u32, Color>,
    jacks: BTreeMap<(&'a str, bool), u32>,
}

impl<'a> Plugs<'a> {
    fn of(snapshot: &'a Snapshot) -> Self {
        let mut plugs = Self::default();
        for cable in &snapshot.cables {
            plugs.hues.insert(cable.id, cable_hue(snapshot, cable.id));
            for end in [(cable.from.as_str(), true), (cable.to.as_str(), false)] {
                let id = plugs.jacks.entry(end).or_insert(cable.id);
                *id = (*id).min(cable.id);
            }
        }
        plugs
    }

    /// The colour of cable `id`.
    fn hue(&self, id: u32) -> Color {
        self.hues
            .get(&id)
            .copied()
            .unwrap_or_else(|| cable_colour(id))
    }

    /// The colour of the lowest-numbered cable in jack `jack`
    /// (`module.name`), if any.
    fn plugged(&self, jack: &str, output: bool) -> Option<Color> {
        self.jacks.get(&(jack, output)).map(|&id| self.hue(id))
    }
}

/// Draw the rack view, and keep its layout for the mouse.
pub fn rack(buf: &mut Buffer, area: Rect, app: &mut App) {
    buf.set_style(area, Style::new().bg(RACK));
    if area.height < 3 || area.width < 3 {
        return;
    }
    if app.snapshot().is_none() {
        waiting(buf, area, app);
        return;
    }
    let mut area = area;
    if !app.link().is_up() {
        banner(buf, area, app);
        area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    }
    if !app.lay_out_rack((area.width, area.height)) {
        put_centred(
            buf,
            area,
            area.y + area.height / 2,
            "the wall is empty: press a to hang a module on it",
            plain(TEXT_DIM, RACK),
        );
        return;
    }
    settle_pan(app, area);
    let window = Window {
        area,
        pan: app.rack.pan,
    };
    let mut paths = std::mem::take(&mut app.rack.paths);
    let (drawn, made) = scene(buf, &window, app, &mut paths);
    app.rack.paths = paths;
    app.rack.cables = drawn;
    app.rack.builds.paths += made;
}

/// Draw the faceplates in view, the cables and the edges. `paths` holds
/// the cables' paths from the frame before, and leaves holding this
/// frame's. Returns the cable drawn in each cell, for the mouse, and how
/// many paths had to be made.
fn scene(
    buf: &mut Buffer,
    window: &Window,
    app: &App,
    paths: &mut BTreeMap<PathKey, Hanging>,
) -> (BTreeMap<Point, u32>, u64) {
    let Some(snapshot) = app.snapshot() else {
        return (BTreeMap::new(), 0);
    };
    let layout = &app.rack.layout;
    let plugs = Plugs::of(snapshot);
    let over = carried_over(app);
    let scene = Scene {
        window,
        app,
        snapshot,
        plugs: &plugs,
        over: over.as_ref(),
    };
    for plate in &layout.plates {
        if !window.shows(plate.at, u32::from(plate.width), u32::from(plate.height)) {
            continue;
        }
        if let Some(module) = snapshot.modules.get(plate.module) {
            faceplate(buf, scene, module, plate);
        }
    }
    for blank in &layout.blanks {
        if window.shows(blank.at, u32::from(blank.width), u32::from(blank.height)) {
            blank_panel(buf, window, blank);
        }
    }
    let drawn = cables(buf, scene, layout, paths);
    edges(buf, window, layout);
    drawn
}

/// A blank panel: a faint dashed outline where a new module can go, a `+`
/// in its middle.
fn blank_panel(buf: &mut Buffer, window: &Window, blank: &Blank) {
    let style = plain(STEEL, RACK);
    let inner = usize::from(blank.width.saturating_sub(2));
    let rule = "┄".repeat(inner);
    for row in 0..u32::from(blank.height) {
        let at = Point {
            y: blank.at.y + row,
            ..blank.at
        };
        let line = if row == 0 {
            format!("┌{rule}┐")
        } else if row + 1 == u32::from(blank.height) {
            format!("└{rule}┘")
        } else {
            format!("┆{}┆", " ".repeat(inner))
        };
        window.put(buf, at, &line, style);
    }
    let middle = Point {
        x: blank.at.x + 1,
        y: blank.at.y + u32::from(blank.height / 2),
    };
    let label = if blank.height >= 3 { "+ add" } else { "+" };
    window.centred(buf, middle, blank.width.saturating_sub(2), label, style);
}

/// Keep the pan on the sheet, put the selection in the middle when asked
/// (`.`, a jump), or bring it into view after a key.
fn settle_pan(app: &mut App, area: Rect) {
    let layout = &app.rack.layout;
    let mut pan = geometry::clamp_pan(app.rack.pan, layout, area.width, area.height);
    if app.rack.centre {
        if let Some(plate) = app.selected_index().and_then(|index| layout.plate(index)) {
            let middle =
                geometry::centred(plate.at, plate.width, plate.height, area.width, area.height);
            pan = geometry::clamp_pan(middle, layout, area.width, area.height);
        }
        app.rack.centre = false;
        app.rack.follow = false;
    }
    if app.rack.follow {
        if let Some(plate) = app.selected_index().and_then(|index| layout.plate(index)) {
            let knob = plate
                .knobs
                .iter()
                .find(|knob| knob.index == app.knob_index());
            let scale = layout.density.scale();
            let (at, width, height) = if plate.height <= area.height {
                (plate.at, plate.width, plate.height)
            } else {
                knob.map_or((plate.at, plate.width, scale.knob_h), |knob| {
                    (knob.at, scale.knob_w, scale.knob_h)
                })
            };
            pan = geometry::reveal(
                pan,
                at,
                width.min(area.width),
                height,
                area.width,
                area.height,
            );
        }
        app.rack.follow = false;
    }
    app.rack.pan = pan;
    app.rack.area = area;
}

/// One module's faceplate.
fn faceplate(buf: &mut Buffer, scene: Scene<'_>, module: &ModuleView, plate: &Plate) {
    let Scene {
        window,
        app,
        snapshot,
        ..
    } = scene;
    let selected = app.selected_index() == Some(plate.module);
    // A faceplate being carried to a new place is edged in amber there.
    let carried = matches!(
        &app.rack.drag,
        Some(Drag::Plate { module: held, preview: Some(_), .. }) if *held == module.id
    );
    let edge = if carried {
        bold(AMBER, CREAM)
    } else if selected {
        bold(BRASS, CREAM)
    } else {
        plain(INK_DIM, CREAM)
    };
    let inner = plate.width - 2;
    let rule = "─".repeat(usize::from(inner));
    // An overview block is small: the cables hang around it, not across
    // it, so its name stays readable (a blank braille cell is one no cable
    // is drawn over).
    let density = app.rack.layout.density;
    let bare = if density == Density::Overview {
        braille(0)
    } else {
        ' '
    };
    let middle = format!("│{}│", bare.to_string().repeat(usize::from(inner)));
    for row in 0..u32::from(plate.height) {
        let at = Point {
            x: plate.at.x,
            y: plate.at.y + row,
        };
        let line = if row == 0 {
            format!("┌{rule}┐")
        } else if row + 1 == u32::from(plate.height) {
            format!("└{rule}┘")
        } else {
            middle.clone()
        };
        window.put(buf, at, &line, edge);
    }
    // Screws, top corners.
    window.put(
        buf,
        Point {
            x: plate.at.x + 1,
            ..plate.at
        },
        "◦",
        edge,
    );
    window.put(
        buf,
        Point {
            x: plate.at.x + u32::from(plate.width) - 2,
            ..plate.at
        },
        "◦",
        edge,
    );
    title(buf, window, snapshot, module, plate, (selected, density));
    if density == Density::Overview {
        strip(buf, scene, module, plate);
        return;
    }
    let scale = density.scale();
    let wall_focus = app.focus() == Focus::Wall;
    for place in &plate.knobs {
        if let Some(knob) = module.knobs.get(place.index) {
            let chosen = selected && wall_focus && app.knob_index() == place.index;
            let hit = Hit::Knob {
                module: plate.module,
                knob: place.index,
            };
            knob_cell(buf, scene, module, knob, place.at, (chosen, &hit), density);
        }
    }
    if let Some(at) = plate.meter {
        meter(buf, window, snapshot, module, at, scale);
    }
    for jack in &plate.jacks {
        let hit = Hit::Jack {
            module: plate.module,
            name: jack.name.clone(),
            output: jack.output,
        };
        socket(buf, scene, module, &hit, jack.at, density);
    }
}

/// An overview block's strip of sockets: a glyph per jack, and one for
/// all the knobs' jacks, filled in the colour of a cable in it.
fn strip(buf: &mut Buffer, scene: Scene<'_>, module: &ModuleView, plate: &Plate) {
    let Scene {
        window,
        plugs,
        over,
        ..
    } = scene;
    for jack in &plate.jacks {
        let hit = Hit::Jack {
            module: plate.module,
            name: jack.name.clone(),
            output: jack.output,
        };
        let signal = scene.app.jack_signal(module, &jack.name, jack.output);
        let colour = plugs.plugged(&format!("{}.{}", module.id, jack.name), jack.output);
        let background = if over == Some(&hit) { AMBER } else { CREAM };
        window.put(
            buf,
            jack.at,
            &jack_glyph(signal, colour.is_some()).to_string(),
            colour.map_or_else(
                || plain(JACK_EMPTY, background),
                |colour| bold(colour, background),
            ),
        );
    }
    if let Some(place) = plate.knobs.first() {
        let colour = module
            .knobs
            .iter()
            .find_map(|knob| plugs.plugged(&format!("{}.{}", module.id, knob.name), false));
        window.put(
            buf,
            place.at,
            &jack_glyph(Signal::Cv, colour.is_some()).to_string(),
            colour.map_or_else(|| plain(JACK_EMPTY, CREAM), |colour| bold(colour, CREAM)),
        );
    }
}

/// Whether module `module` has faulted lately.
fn faulted(snapshot: &Snapshot, module: &ModuleView) -> bool {
    snapshot
        .faults
        .recent
        .iter()
        .any(|fault| fault.starts_with(&format!("{} ", module.id)))
}

/// The faceplate's title: its name, then its kind (and id, when it has a
/// name) or its fault; on one line when compact, the name alone (or the
/// fault).
fn title(
    buf: &mut Buffer,
    window: &Window,
    snapshot: &Snapshot,
    module: &ModuleView,
    plate: &Plate,
    (selected, density): (bool, Density),
) {
    let inner = plate.width - 2;
    let x = plate.at.x + 1;
    let name = module.name.as_deref().unwrap_or(&module.id);
    let name_style = if selected {
        bold(INK, BRASS)
    } else {
        bold(INK, CREAM)
    };
    let faulted = faulted(snapshot, module);
    let name_at = Point {
        x,
        y: plate.at.y + 1,
    };
    if density.scale().title_h < 2 {
        let (text, style) = if faulted {
            (
                format!("⚠ {name}"),
                bold(RED, if selected { BRASS } else { CREAM }),
            )
        } else {
            (format!(" {name} "), name_style)
        };
        window.centred(buf, name_at, inner, &text, style);
        return;
    }
    // The overview's name is not padded: nothing hangs across a block.
    let padded = if density == Density::Overview {
        name.to_string()
    } else {
        format!(" {name} ")
    };
    window.centred(buf, name_at, inner, &padded, name_style);
    let (under, style) = match (&module.name, faulted) {
        (_, true) => (format!("{} ⚠ fault", module.kind), bold(RED, CREAM)),
        (Some(_), false) => (
            format!("{} · {}", module.kind, module.id),
            plain(INK_DIM, CREAM),
        ),
        (None, false) => (module.kind.clone(), plain(INK_DIM, CREAM)),
    };
    window.centred(
        buf,
        Point {
            x,
            y: plate.at.y + 2,
        },
        inner,
        &under,
        style,
    );
}

/// What the mouse is carrying a cable over, if it is carrying one.
fn carried_over(app: &App) -> Option<Hit> {
    let Some(Drag::Cable { pointer, .. }) = &app.rack.drag else {
        return None;
    };
    app.rack
        .to_sheet(*pointer)
        .map(|point| app.rack.layout.hit(point))
}

/// A dial's cells from `at` (a column in): the pointer red on the chosen
/// knob.
fn draw_dial(
    buf: &mut Buffer,
    window: &Window,
    at: Point,
    cells: &[Vec<dial::Cell>],
    chosen: bool,
) {
    for (row, line) in cells.iter().enumerate() {
        for (col, cell) in line.iter().enumerate() {
            let colour = match cell.part {
                Part::Blank | Part::Ring => INK_DIM,
                Part::Marker => AMBER,
                Part::Pointer => {
                    if chosen {
                        RED
                    } else {
                        KNOB
                    }
                }
            };
            let style = if cell.part == Part::Pointer {
                bold(colour, CREAM)
            } else {
                plain(colour, CREAM)
            };
            window.put(
                buf,
                Point {
                    x: at.x + 1 + u32::try_from(col).unwrap_or(0),
                    y: at.y + u32::try_from(row).unwrap_or(0),
                },
                &cell.glyph.to_string(),
                style,
            );
        }
    }
}

/// One knob: its dial (small when compact), name and value. `chosen` is
/// whether it is the selected knob; `hit` is what the mouse finds there.
fn knob_cell(
    buf: &mut Buffer,
    scene: Scene<'_>,
    module: &ModuleView,
    knob: &KnobView,
    at: Point,
    (chosen, hit): (bool, &Hit),
    density: Density,
) {
    let Scene {
        window,
        app,
        plugs,
        over,
        ..
    } = scene;
    let info = app.knob_info(&module.kind, &knob.name);
    let travel = Travel::of(knob, info);
    let ahead = app.ahead(&module.id, &knob.name);
    // The pointer is where the knob is, or where Tom is turning it; the
    // marker is where a glide is heading.
    let pointer = ahead.unwrap_or(knob.value);
    let gliding = ahead.is_none() && !travel.same(knob.value, knob.target);
    let moving = gliding || ahead.is_some_and(|ahead| !travel.same(ahead, knob.value));
    let target = gliding.then(|| travel.position(knob.target));
    let steps = travel
        .stepped
        .then(|| (travel.max - travel.min).round() as usize + 1);
    let scale = density.scale();
    let position = travel.position(pointer);
    let cells: Vec<Vec<dial::Cell>> = if density == Density::Compact {
        dial::small(position, steps, target)
            .iter()
            .map(|line| line.to_vec())
            .collect()
    } else {
        dial::dial(position, steps, target)
            .iter()
            .map(|line| line.to_vec())
            .collect()
    };
    let dial_rows = cells.len();
    draw_dial(buf, window, at, &cells, chosen);
    // The knob's own jack, when a cable is in it.
    let jack = format!("{}.{}", module.id, knob.name);
    let over = over == Some(hit);
    if let Some(colour) = plugs.plugged(&jack, false) {
        window.put(
            buf,
            at,
            &jack_glyph(Signal::Cv, true).to_string(),
            bold(colour, CREAM),
        );
    } else if over {
        window.put(
            buf,
            at,
            &jack_glyph(Signal::Cv, false).to_string(),
            bold(AMBER, CREAM),
        );
    }
    let label_style = if chosen || over {
        bold(INK, if over { AMBER } else { BRASS })
    } else {
        plain(INK, CREAM)
    };
    let label_at = Point {
        x: at.x,
        y: at.y + u32::try_from(dial_rows).unwrap_or(3),
    };
    // Compact knobs sit close: their names leave a column between them.
    let label_width = if density == Density::Compact {
        scale.knob_w - 1
    } else {
        scale.knob_w
    };
    window.centred(buf, label_at, label_width, &knob.name, label_style);
    let labels = info.map_or(&[][..], |info| info.labels.as_slice());
    let value = match ahead {
        Some(ahead) => show(ahead, &knob.unit, labels, travel.min),
        None if gliding => knob.target_display.clone(),
        None => knob.display.clone(),
    };
    let value_style = if moving {
        bold(AMBER, CREAM)
    } else {
        plain(INK, CREAM)
    };
    window.centred(
        buf,
        Point {
            y: label_at.y + 1,
            ..label_at
        },
        scale.knob_w,
        &value,
        value_style,
    );
}

/// An `out` module's meter: two bars, left and right.
fn meter(
    buf: &mut Buffer,
    window: &Window,
    snapshot: &Snapshot,
    module: &ModuleView,
    at: Point,
    scale: Scale,
) {
    let height = u32::from(scale.knob_rows * scale.knob_h) - 1;
    let levels = super::out_levels(snapshot, &module.id);
    for (column, level) in [
        (1_u32, levels.map(|(left, _)| left)),
        (2, levels.map(|(_, right)| right)),
    ] {
        let fraction = level.filter(|db| db.is_finite()).map_or(0.0, |db| {
            ((db - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
        });
        let eighths = (fraction * f64::from(height) * 8.0).round() as u32;
        for row in 0..height {
            let from_bottom = height - 1 - row;
            let filled = eighths.saturating_sub(from_bottom * 8).min(8);
            let cell_db = METER_FLOOR * (1.0 - f64::from(from_bottom + 1) / f64::from(height));
            let colour = if cell_db > -3.0 {
                RED
            } else if cell_db > -12.0 {
                AMBER
            } else {
                SAGE
            };
            window.put(
                buf,
                Point {
                    x: at.x + column,
                    y: at.y + row,
                },
                &EIGHTHS[filled as usize].to_string(),
                plain(colour, METER_BG),
            );
        }
    }
    let label = if levels.is_some() { "LR" } else { "--" };
    window.put(
        buf,
        Point {
            x: at.x + 1,
            y: at.y + height,
        },
        label,
        plain(INK_DIM, CREAM),
    );
}

/// One jack (`hit`, a [`Hit::Jack`]): a socket, and its name (outputs
/// printed reversed, as on hardware): under it, or beside it when compact.
fn socket(
    buf: &mut Buffer,
    scene: Scene<'_>,
    module: &ModuleView,
    hit: &Hit,
    at: Point,
    density: Density,
) {
    let Scene {
        window,
        app,
        plugs,
        over,
        ..
    } = scene;
    let Hit::Jack { name, output, .. } = hit else {
        return;
    };
    let (name, output) = (name.as_str(), *output);
    let jack = format!("{}.{name}", module.id);
    let signal = app.jack_signal(module, name, output);
    let colour = plugs.plugged(&jack, output);
    let over = over == Some(hit);
    let ring = if over {
        bold(AMBER, CREAM)
    } else {
        plain(INK_DIM, CREAM)
    };
    let scale = density.scale();
    let x = at.x + scale.socket_dx - 1;
    window.put(buf, Point { x, ..at }, "(", ring);
    window.put(
        buf,
        Point { x: x + 1, ..at },
        &jack_glyph(signal, colour.is_some()).to_string(),
        colour.map_or_else(|| plain(JACK_EMPTY, CREAM), |colour| bold(colour, CREAM)),
    );
    window.put(buf, Point { x: x + 2, ..at }, ")", ring);
    let name_style = match (over, output) {
        (true, _) => bold(INK, AMBER),
        (false, true) => plain(CREAM, INK),
        (false, false) => plain(INK, CREAM),
    };
    if scale.jack_h > 1 {
        window.centred(
            buf,
            Point { y: at.y + 1, ..at },
            scale.jack_w,
            name,
            name_style,
        );
    } else {
        let room = usize::from(scale.jack_w.saturating_sub(3));
        window.put(
            buf,
            Point { x: x + 3, ..at },
            &truncate(name, room),
            name_style,
        );
    }
}

/// The cables, hanging over the rack where nothing is printed, and the
/// cable the mouse is carrying. `paths` holds the paths from the frame
/// before and leaves holding this frame's; a cable wholly out of view is
/// not drawn. Returns the cable shown in each cell (the latest-plugged on
/// top), and how many paths had to be made.
fn cables(
    buf: &mut Buffer,
    scene: Scene<'_>,
    layout: &Rack,
    paths: &mut BTreeMap<PathKey, Hanging>,
) -> (BTreeMap<Point, u32>, u64) {
    let Scene {
        window,
        app,
        snapshot,
        plugs,
        ..
    } = scene;
    let mut ends = geometry::cable_ends(layout, &snapshot.modules, &snapshot.cables);
    ends.sort_by_key(|(id, ..)| *id);
    let mut before = std::mem::take(paths);
    let mut made = 0;
    let mut drawn = Drawn::default();
    for &(id, from, to) in &ends {
        let key = (id, from, to);
        let path = before.remove(&key).unwrap_or_else(|| {
            made += 1;
            geometry::hanging(from, to)
        });
        let (low, high) = (path.low, path.high);
        if window.shows(low, high.x - low.x + 1, high.y - low.y + 1) {
            hang(
                buf,
                window,
                &mut drawn,
                &path.cells,
                plugs.hue(id),
                Some(id),
            );
        }
        paths.insert(key, path);
    }
    if let Some(Drag::Cable {
        module,
        name,
        output,
        pointer,
    }) = &app.rack.drag
    {
        let start = snapshot
            .modules
            .iter()
            .position(|view| view.id == *module)
            .and_then(|index| {
                layout
                    .plate(index)?
                    .socket(name, *output, &snapshot.modules[index], layout.density)
            });
        if let (Some(start), Some(end)) = (start, app.rack.to_sheet(*pointer)) {
            let carried: Vec<(Point, u8)> = geometry::cable_path(start, end).into_iter().collect();
            hang(buf, window, &mut drawn, &carried, BRASS, None);
        }
    }
    (drawn.cables, made)
}

/// What the cables have drawn so far: the dots in each screen cell, and
/// the cable shown in each sheet cell.
#[derive(Default)]
struct Drawn {
    dots: BTreeMap<(u16, u16), u8>,
    cables: BTreeMap<Point, u32>,
}

/// One cable along `path` in `colour`, drawn only where the rack or
/// faceplate is bare (or another cable already runs). `id` is the cable's
/// number, recorded for the mouse; `None` for the cable being carried.
fn hang(
    buf: &mut Buffer,
    window: &Window,
    drawn: &mut Drawn,
    path: &[(Point, u8)],
    colour: Color,
    id: Option<u32>,
) {
    for &(point, bits) in path {
        let Some(cell) = window.screen(point) else {
            continue;
        };
        let earlier = drawn.dots.get(&cell).copied();
        if earlier.is_none() && buf[cell].symbol() != " " {
            continue;
        }
        let merged = earlier.unwrap_or(0) | bits;
        buf[cell].set_char(braille(merged)).set_fg(colour);
        drawn.dots.insert(cell, merged);
        if let Some(id) = id {
            drawn.cables.insert(point, id);
        }
    }
}

/// Arrows at the edges where the rack runs on out of view.
fn edges(buf: &mut Buffer, window: &Window, layout: &Rack) {
    let area = window.area;
    let style = bold(BRASS, RACK);
    let middle_row = area.y + area.height / 2;
    let middle_column = area.x + area.width / 2;
    if window.pan.x > 0 {
        buf[(area.x, middle_row)].set_char('◀').set_style(style);
    }
    if window.pan.x + u32::from(area.width) < layout.width {
        buf[(area.right() - 1, middle_row)]
            .set_char('▶')
            .set_style(style);
    }
    if window.pan.y > 0 {
        buf[(middle_column, area.y)].set_char('▲').set_style(style);
    }
    if window.pan.y + u32::from(area.height) < layout.height {
        buf[(middle_column, area.bottom() - 1)]
            .set_char('▼')
            .set_style(style);
    }
}
