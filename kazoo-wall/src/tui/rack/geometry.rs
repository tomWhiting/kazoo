//! Where everything sits in the rack view, on a sheet of terminal cells
//! the screen pans across.
//!
//! Every module is a faceplate as tall as its row of the rack, like a
//! Eurorack case: a title, its knobs in columns of [`KNOB_ROWS`], a meter
//! if it is an `out`, and its jacks along the bottom, outputs below inputs.
//! The rack's rows are the wall's own, shared by every console like a
//! real case, and it pans the rest of the way, like walking along the
//! wall. (From a wall older than the rows, it wraps into as many rows as
//! fit the screen's height, each about the same width.)
//!
//! The rack is drawn at one of three [`Density`] levels: full faceplates;
//! compact ones (small dials, one title line, one-line jacks); or an
//! overview where each module is a small block with its sockets, and the
//! cables still hang between them.
//!
//! The same geometry answers where the mouse is ([`Rack::hit`]) and where
//! the cables run ([`cable_path`]), so what is drawn is what is clicked.

use std::collections::BTreeMap;

use kazoo_wall::protocol::{CableRecord, ModuleView};

use super::dial;

/// Width of one knob: its dial with a column either side.
pub const KNOB_W: u16 = dial::COLS as u16 + 2;

/// Height of one knob: the dial, its name and its value.
pub const KNOB_H: u16 = dial::ROWS as u16 + 2;

/// Knobs in each column of a faceplate.
pub const KNOB_ROWS: u16 = 3;

/// Width of one jack: its socket and its name.
pub const JACK_W: u16 = 6;

/// Height of one jack: the socket, then the name.
pub const JACK_H: u16 = 2;

/// Title lines: the module's name, then its kind and id.
pub const TITLE_H: u16 = 2;

/// Width of an `out` module's meter.
pub const METER_W: u16 = 4;

/// Narrowest faceplate inside.
pub const MIN_INNER: u16 = 12;

/// Columns between faceplates, and at the rack's left edge.
pub const GAP_X: u16 = 1;

/// Rows between rows of the rack.
pub const GAP_Y: u16 = 1;

/// How densely the rack is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Density {
    /// Full faceplates: big dials, the name and the kind, two-line jacks.
    #[default]
    Full,
    /// Smaller faceplates: small dials with every knob's name and value,
    /// one title line, one-line jacks.
    Compact,
    /// Each module a small block (its name, its kind and a strip of
    /// sockets), the cables still hanging between them.
    Overview,
}

impl Density {
    /// The next level `c` goes to: full, compact, overview, and round.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Full => Self::Compact,
            Self::Compact => Self::Overview,
            Self::Overview => Self::Full,
        }
    }

    /// The level in a word.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Overview => "overview",
        }
    }

    /// The sizes faceplates are laid out with at this level.
    #[must_use]
    pub const fn scale(self) -> Scale {
        match self {
            Self::Full => Scale {
                knob_w: KNOB_W,
                knob_h: KNOB_H,
                knob_rows: KNOB_ROWS,
                jack_w: JACK_W,
                jack_h: JACK_H,
                jacks_across: 2,
                title_h: TITLE_H,
                min_inner: MIN_INNER,
                socket_dx: 2,
            },
            Self::Compact => Scale {
                knob_w: dial::SMALL_COLS as u16 + 2,
                knob_h: dial::SMALL_ROWS as u16 + 2,
                knob_rows: KNOB_ROWS,
                jack_w: 8,
                jack_h: 1,
                jacks_across: 1,
                title_h: 1,
                min_inner: 10,
                socket_dx: 1,
            },
            // No knobs to hit: their jack is one socket in the strip.
            Self::Overview => Scale {
                knob_w: 0,
                knob_h: 0,
                knob_rows: KNOB_ROWS,
                jack_w: 1,
                jack_h: 1,
                jacks_across: 0,
                title_h: 2,
                min_inner: OVERVIEW_INNER,
                socket_dx: 0,
            },
        }
    }
}

/// The sizes a faceplate is laid out with (see [`Density::scale`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    /// Width of one knob: its dial with a column either side.
    pub knob_w: u16,
    /// Height of one knob: the dial, its name and its value.
    pub knob_h: u16,
    /// Knobs in each column.
    pub knob_rows: u16,
    /// Width of one jack.
    pub jack_w: u16,
    /// Height of one jack.
    pub jack_h: u16,
    /// Jacks a faceplate is made wide enough to hold side by side.
    pub jacks_across: u16,
    /// Title lines.
    pub title_h: u16,
    /// Narrowest faceplate inside.
    pub min_inner: u16,
    /// Columns from a jack's top-left to its socket.
    pub socket_dx: u32,
}

/// Narrowest overview block inside.
pub const OVERVIEW_INNER: u16 = 10;

/// Height of an overview block: its edges, name, kind and sockets.
pub const OVERVIEW_H: u16 = 5;

/// Width of a blank panel.
pub const BLANK_W: u16 = 12;

/// Height of the spare row's blank panel.
pub const SPARE_H: u16 = 5;

/// A cell on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Point {
    /// Column.
    pub x: u32,
    /// Row.
    pub y: u32,
}

/// A knob's place: the top-left of its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnobPlace {
    /// The knob's index in the module.
    pub index: usize,
    /// Top-left.
    pub at: Point,
}

/// A jack's place: the top-left of its cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackPlace {
    /// Its name.
    pub name: String,
    /// An output (rather than an input).
    pub output: bool,
    /// Top-left.
    pub at: Point,
}

/// One faceplate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plate {
    /// The module's index in the snapshot.
    pub module: usize,
    /// Top-left.
    pub at: Point,
    /// Width, border included.
    pub width: u16,
    /// Height, border included.
    pub height: u16,
    /// Its knobs.
    pub knobs: Vec<KnobPlace>,
    /// Its jacks, inputs then outputs.
    pub jacks: Vec<JackPlace>,
    /// The meter's top-left, for an `out`.
    pub meter: Option<Point>,
}

impl Plate {
    /// Whether `point` is on the faceplate.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        within(point, self.at, self.width, self.height)
    }

    /// Where a cable meets jack `name` at `density`: a socket's centre, or
    /// the corner of a knob whose jack it is (in the overview, the one
    /// socket all the knobs' jacks share).
    #[must_use]
    pub fn socket(
        &self,
        name: &str,
        output: bool,
        module: &ModuleView,
        density: Density,
    ) -> Option<Point> {
        if let Some(jack) = self
            .jacks
            .iter()
            .find(|jack| jack.name == name && jack.output == output)
        {
            return Some(Point {
                x: jack.at.x + density.scale().socket_dx,
                y: jack.at.y,
            });
        }
        if output {
            return None;
        }
        let index = module.knobs.iter().position(|knob| knob.name == name)?;
        self.knobs
            .iter()
            .find(|place| place.index == index)
            .map(|place| place.at)
    }
}

/// Where a faceplate carried by the mouse would go: in row `row` (or a
/// new row of its own there), in front of module `before` or at the row's
/// end. Rows are the layout's own, top to bottom; one past the last is a
/// new bottom row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// The row.
    pub row: usize,
    /// The module, by index, it goes in front of.
    pub before: Option<usize>,
    /// A new row of its own at `row`.
    pub own: bool,
}

/// What is under the mouse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// A knob.
    Knob {
        /// The module's index.
        module: usize,
        /// The knob's index.
        knob: usize,
    },
    /// A jack.
    Jack {
        /// The module's index.
        module: usize,
        /// The jack's name.
        name: String,
        /// An output.
        output: bool,
    },
    /// A faceplate's title bar: its top edge and its name, where it is
    /// picked up to move it.
    Title {
        /// The module's index.
        module: usize,
    },
    /// Elsewhere on a faceplate.
    Plate {
        /// The module's index.
        module: usize,
    },
    /// A blank panel: at the end of row `row`, or (one past the last) the
    /// spare row below them, where a new module can go.
    Blank {
        /// The row.
        row: usize,
    },
    /// The rack between faceplates.
    Background,
}

/// A blank panel, where a new module can go: one at the end of each row,
/// and one on a spare row below the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blank {
    /// The row it ends (one past the last for the spare row).
    pub row: usize,
    /// Top-left.
    pub at: Point,
    /// Width.
    pub width: u16,
    /// Height.
    pub height: u16,
}

/// The whole rack.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rack {
    /// One faceplate per module, in the snapshot's order.
    pub plates: Vec<Plate>,
    /// Width of the sheet.
    pub width: u32,
    /// Height of the sheet.
    pub height: u32,
    /// How densely it is laid out.
    pub density: Density,
    /// Each row of faceplates, top to bottom: its top and its height.
    pub bands: Vec<(u32, u16)>,
    /// The blank panels, in the wall's own rows (a wall older than the rows
    /// has none: it places new modules itself).
    pub blanks: Vec<Blank>,
}

fn within(point: Point, at: Point, width: u16, height: u16) -> bool {
    point.x >= at.x
        && point.y >= at.y
        && point.x < at.x + u32::from(width)
        && point.y < at.y + u32::from(height)
}

/// Whether `module` has a meter.
#[must_use]
pub fn has_meter(module: &ModuleView) -> bool {
    module.kind == "out"
}

fn count(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX / 16)
}

/// Knob columns a module needs.
fn knob_columns(module: &ModuleView, scale: Scale) -> u16 {
    count(module.knobs.len()).div_ceil(scale.knob_rows)
}

/// The width inside a module's faceplate.
fn inner_width(module: &ModuleView, scale: Scale) -> u16 {
    let knobs =
        knob_columns(module, scale) * scale.knob_w + if has_meter(module) { METER_W } else { 0 };
    let across = usize::from(scale.jacks_across);
    let jacks = count(module.inputs.len().max(module.outputs.len()).min(across)) * scale.jack_w;
    knobs.max(jacks).max(scale.min_inner)
}

/// Jacks across a faceplate `inner` wide.
const fn jack_columns(inner: u16, scale: Scale) -> u16 {
    let columns = inner / scale.jack_w;
    if columns == 0 { 1 } else { columns }
}

/// Lines of jacks a module needs.
fn jack_rows(module: &ModuleView, inner: u16, scale: Scale) -> u16 {
    let columns = jack_columns(inner, scale);
    count(module.inputs.len()).div_ceil(columns) + count(module.outputs.len()).div_ceil(columns)
}

/// The overview's strip of sockets: each input, one socket for every
/// knob's jack, a gap, then each output. Its length.
fn strip_len(module: &ModuleView) -> u16 {
    let knobs = u16::from(!module.knobs.is_empty());
    let outputs = if module.outputs.is_empty() {
        0
    } else {
        1 + count(module.outputs.len())
    };
    count(module.inputs.len()) + knobs + outputs
}

/// A module's faceplate size at `density`: width, and the least height it
/// needs.
#[must_use]
pub fn plate_size(module: &ModuleView, density: Density) -> (u16, u16) {
    let scale = density.scale();
    if density == Density::Overview {
        let inner = strip_len(module).max(scale.min_inner);
        return (inner + 2, OVERVIEW_H);
    }
    let inner = inner_width(module, scale);
    let height = 2
        + scale.title_h
        + scale.knob_rows * scale.knob_h
        + jack_rows(module, inner, scale) * scale.jack_h;
    (inner + 2, height)
}

/// Lay the rack out at `density` for a view `(width, height)`.
///
/// With `rows` (the wall's own, by module id), each row of the rack is one
/// of them, in order: ids not on the wall are passed over, and modules in
/// no row hang in a row of their own at the bottom. Without them (a wall
/// older than the rows), the modules wrap in the order they were added
/// into as many rows as fit the view's height, each about the same width
/// (the overview's small blocks in no more rows than it takes to fill its
/// width).
#[must_use]
pub fn rack(
    modules: &[ModuleView],
    density: Density,
    view: (u16, u16),
    rows: Option<&[Vec<String>]>,
) -> Rack {
    if modules.is_empty() {
        // No faceplates, so no margin around them either.
        return Rack {
            density,
            ..Rack::default()
        };
    }
    let sizes: Vec<(u16, u16)> = modules
        .iter()
        .map(|module| plate_size(module, density))
        .collect();
    let (sequence, count) = rows.map_or_else(
        || wrapped(&sizes, density, view),
        |rows| given(modules, rows),
    );

    let mut heights = vec![0_u16; count];
    for &(index, row) in &sequence {
        heights[row] = heights[row].max(sizes[index].1);
    }
    let mut tops = Vec::with_capacity(count);
    let mut bands = Vec::with_capacity(count);
    let mut top = 0_u32;
    for &height in &heights {
        tops.push(top);
        if height > 0 {
            bands.push((top, height));
            top += u32::from(height + GAP_Y);
        }
    }

    let mut lefts = vec![u32::from(GAP_X); count];
    let mut plates = Vec::with_capacity(modules.len());
    let given = rows.is_some();
    for &(index, row) in &sequence {
        let at = Point {
            x: lefts[row],
            y: tops[row],
        };
        let (width, _) = sizes[index];
        lefts[row] += u32::from(width + GAP_X);
        let size = (width, heights[row]);
        plates.push(if density == Density::Overview {
            block(index, &modules[index], at, size)
        } else {
            place(index, &modules[index], at, size, density.scale())
        });
    }
    // A blank panel ends each of the wall's rows, and a spare row waits
    // below them.
    let mut blanks = Vec::new();
    if given {
        for (row, &(band_top, height)) in bands.iter().enumerate() {
            let left = &mut lefts[row];
            blanks.push(Blank {
                row,
                at: Point {
                    x: *left,
                    y: band_top,
                },
                width: BLANK_W,
                height,
            });
            *left += u32::from(BLANK_W + GAP_X);
        }
        blanks.push(Blank {
            row: bands.len(),
            at: Point {
                x: u32::from(GAP_X),
                y: top,
            },
            width: BLANK_W,
            height: SPARE_H,
        });
        top += u32::from(SPARE_H + GAP_Y);
    }
    Rack {
        plates,
        width: lefts.iter().copied().max().unwrap_or(0),
        height: top.saturating_sub(u32::from(GAP_Y)),
        density,
        bands,
        blanks,
    }
}

/// The modules in the wall's own `rows`: each module's index and row, in
/// order along the rows, and how many rows. Ids not on the wall, and second
/// mentions, are passed over; modules in no row get one of their own at the
/// bottom.
fn given(modules: &[ModuleView], rows: &[Vec<String>]) -> (Vec<(usize, usize)>, usize) {
    let index: BTreeMap<&str, usize> = modules
        .iter()
        .enumerate()
        .map(|(at, module)| (module.id.as_str(), at))
        .collect();
    let mut hung = vec![false; modules.len()];
    let mut sequence = Vec::with_capacity(modules.len());
    let mut count = 0;
    for ids in rows {
        let before = sequence.len();
        for id in ids {
            if let Some(&at) = index.get(id.as_str()) {
                if !hung[at] {
                    hung[at] = true;
                    sequence.push((at, count));
                }
            }
        }
        if sequence.len() > before {
            count += 1;
        }
    }
    if hung.iter().any(|hung| !hung) {
        for (at, _) in hung.iter().enumerate().filter(|(_, hung)| !**hung) {
            sequence.push((at, count));
        }
        count += 1;
    }
    (sequence, count)
}

/// The modules wrapped, in the order they were added, into as many rows as
/// fit the view: each module's index and row, and how many rows.
fn wrapped(
    sizes: &[(u16, u16)],
    density: Density,
    (view_width, view_height): (u16, u16),
) -> (Vec<(usize, usize)>, usize) {
    let tallest = sizes.iter().map(|&(_, height)| height).max().unwrap_or(0);
    let fit = usize::from(((view_height + GAP_Y) / (tallest + GAP_Y)).max(1));
    let total: u32 = sizes
        .iter()
        .map(|&(width, _)| u32::from(width + GAP_X))
        .sum();
    let rows = if density == Density::Overview {
        let across = total.div_ceil(u32::from(view_width.max(1)));
        fit.min(usize::try_from(across).unwrap_or(fit).max(1))
    } else {
        fit
    };
    let target = total.div_ceil(u32::try_from(rows).unwrap_or(1));

    // Split into rows of about the same width, in order.
    let mut sequence = Vec::with_capacity(sizes.len());
    let mut row = 0_usize;
    let mut used = 0_u32;
    for (index, &(width, _)) in sizes.iter().enumerate() {
        let width = u32::from(width + GAP_X);
        // A faceplate starts the next row when more than half of it would
        // run past this row's share.
        if used > 0 && used + width / 2 > target && row + 1 < rows {
            row += 1;
            used = 0;
        }
        used += width;
        sequence.push((index, row));
    }
    (sequence, rows)
}

/// Place a module's knobs, meter and jacks on a faceplate at `at`, `width`
/// × `height`, laid out with `scale`.
fn place(
    index: usize,
    module: &ModuleView,
    at: Point,
    (width, height): (u16, u16),
    scale: Scale,
) -> Plate {
    let inner_x = at.x + 1;
    let knobs_y = at.y + 1 + u32::from(scale.title_h);
    let knobs = (0..module.knobs.len())
        .map(|knob| {
            let knob_u32 = u32::try_from(knob).unwrap_or(u32::MAX / 64);
            let rows = u32::from(scale.knob_rows);
            KnobPlace {
                index: knob,
                at: Point {
                    x: inner_x + knob_u32 / rows * u32::from(scale.knob_w),
                    y: knobs_y + knob_u32 % rows * u32::from(scale.knob_h),
                },
            }
        })
        .collect();
    let meter = has_meter(module).then(|| Point {
        x: inner_x + u32::from(knob_columns(module, scale) * scale.knob_w),
        y: knobs_y,
    });
    let inner = width - 2;
    let columns = u32::from(jack_columns(inner, scale));
    // Jacks sit along the bottom, as on hardware.
    let mut y =
        at.y + u32::from(height) - 1 - u32::from(jack_rows(module, inner, scale) * scale.jack_h);
    let mut jacks = Vec::with_capacity(module.inputs.len() + module.outputs.len());
    for (names, output) in [(&module.inputs, false), (&module.outputs, true)] {
        for (slot, name) in names.iter().enumerate() {
            let slot = u32::try_from(slot).unwrap_or(u32::MAX / 64);
            jacks.push(JackPlace {
                name: name.clone(),
                output,
                at: Point {
                    x: inner_x + slot % columns * u32::from(scale.jack_w),
                    y: y + slot / columns * u32::from(scale.jack_h),
                },
            });
        }
        let lines = u32::from(count(names.len())).div_ceil(columns);
        y += lines * u32::from(scale.jack_h);
    }
    Plate {
        module: index,
        at,
        width,
        height,
        knobs,
        jacks,
        meter,
    }
}

/// Place an overview block at `at`, `width` × `height`: its sockets in a
/// strip along the bottom (inputs, the knobs' shared jack, a gap, then
/// outputs), every knob's jack at that one socket.
fn block(index: usize, module: &ModuleView, at: Point, (width, height): (u16, u16)) -> Plate {
    let y = at.y + u32::from(height) - 2;
    let mut x = at.x + 1;
    let mut jacks = Vec::with_capacity(module.inputs.len() + module.outputs.len());
    for name in &module.inputs {
        jacks.push(JackPlace {
            name: name.clone(),
            output: false,
            at: Point { x, y },
        });
        x += 1;
    }
    let knob_socket = Point { x, y };
    let knobs = (0..module.knobs.len())
        .map(|knob| KnobPlace {
            index: knob,
            at: knob_socket,
        })
        .collect();
    if !module.knobs.is_empty() {
        x += 1;
    }
    // A gap between what goes in and what comes out.
    x += 1;
    for name in &module.outputs {
        jacks.push(JackPlace {
            name: name.clone(),
            output: true,
            at: Point { x, y },
        });
        x += 1;
    }
    Plate {
        module: index,
        at,
        width,
        height,
        knobs,
        jacks,
        meter: None,
    }
}

impl Rack {
    /// What is at `point` on the sheet.
    #[must_use]
    pub fn hit(&self, point: Point) -> Hit {
        let Some(plate) = self.plates.iter().find(|plate| plate.contains(point)) else {
            return self
                .blanks
                .iter()
                .find(|blank| within(point, blank.at, blank.width, blank.height))
                .map_or(Hit::Background, |blank| Hit::Blank { row: blank.row });
        };
        let scale = self.density.scale();
        if point.y < plate.at.y + 1 + u32::from(scale.title_h) {
            return Hit::Title {
                module: plate.module,
            };
        }
        if let Some(knob) = plate
            .knobs
            .iter()
            .find(|knob| within(point, knob.at, scale.knob_w, scale.knob_h))
        {
            return Hit::Knob {
                module: plate.module,
                knob: knob.index,
            };
        }
        if let Some(jack) = plate
            .jacks
            .iter()
            .find(|jack| within(point, jack.at, scale.jack_w, scale.jack_h))
        {
            return Hit::Jack {
                module: plate.module,
                name: jack.name.clone(),
                output: jack.output,
            };
        }
        Hit::Plate {
            module: plate.module,
        }
    }

    /// The faceplate of module `index`.
    #[must_use]
    pub fn plate(&self, index: usize) -> Option<&Plate> {
        self.plates.iter().find(|plate| plate.module == index)
    }

    /// Where a faceplate dropped at `point` goes: in the row whose band
    /// holds it, in front of the first faceplate whose middle is right of
    /// it (or at the row's end); on the gap between two rows, a row of its
    /// own there; below the last row, a new bottom row.
    #[must_use]
    pub fn slot_at(&self, point: Point) -> Slot {
        for (row, &(top, height)) in self.bands.iter().enumerate() {
            if point.y < top {
                // Above the first row is the first row; between two, a
                // row of its own.
                return Slot {
                    row,
                    before: None,
                    own: row > 0,
                };
            }
            if point.y < top + u32::from(height) {
                let before = self
                    .plates
                    .iter()
                    .filter(|plate| plate.at.y == top)
                    .filter(|plate| plate.at.x * 2 + u32::from(plate.width) > point.x * 2)
                    .min_by_key(|plate| plate.at.x)
                    .map(|plate| plate.module);
                return Slot {
                    row,
                    before,
                    own: false,
                };
            }
        }
        Slot {
            row: self.bands.len(),
            before: None,
            own: false,
        }
    }

    /// The module after module `index` on screen (or before it): along its
    /// row, then on to the start of the next row (or the end of the one
    /// before). Module `index` itself at either end of the rack; `None`
    /// when it has no faceplate.
    #[must_use]
    pub fn across(&self, index: usize, right: bool) -> Option<usize> {
        let from = self.plate(index)?;
        let key = |plate: &Plate| (plate.at.y, plate.at.x);
        let next = if right {
            self.plates
                .iter()
                .filter(|plate| key(plate) > key(from))
                .min_by_key(|plate| key(plate))
        } else {
            self.plates
                .iter()
                .filter(|plate| key(plate) < key(from))
                .max_by_key(|plate| key(plate))
        };
        Some(next.map_or(index, |plate| plate.module))
    }

    /// The module on the row below module `index` (or above), nearest to
    /// straight down (or up) from the middle of its faceplate.
    #[must_use]
    pub fn beside(&self, index: usize, below: bool) -> Option<usize> {
        let from = self.plate(index)?;
        let row = self
            .plates
            .iter()
            .map(|plate| plate.at.y)
            .filter(|&y| if below { y > from.at.y } else { y < from.at.y });
        let row = if below { row.min() } else { row.max() }?;
        let middle = |plate: &Plate| plate.at.x * 2 + u32::from(plate.width);
        self.plates
            .iter()
            .filter(|plate| plate.at.y == row)
            .min_by_key(|plate| middle(plate).abs_diff(middle(from)))
            .map(|plate| plate.module)
    }
}

/// What a faceplate is laid out from. Everything else about a module (its
/// name, its knobs' values) is drawn on the faceplate but moves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shape {
    knobs: usize,
    inputs: Vec<String>,
    outputs: Vec<String>,
    meter: bool,
}

impl Shape {
    fn of(module: &ModuleView) -> Self {
        Self {
            knobs: module.knobs.len(),
            inputs: module.inputs.clone(),
            outputs: module.outputs.clone(),
            meter: has_meter(module),
        }
    }

    fn fits(&self, module: &ModuleView) -> bool {
        self.knobs == module.knobs.len()
            && self.inputs == module.inputs
            && self.outputs == module.outputs
            && self.meter == has_meter(module)
    }
}

/// What a layout was made from, so it is made again only when one of
/// those changes, not on every frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LayoutKey {
    shapes: Vec<Shape>,
    density: Density,
    view: (u16, u16),
    rows: Option<Vec<Vec<String>>>,
}

impl LayoutKey {
    /// The key of a layout of `modules` at `density` for a view `view`
    /// (width, height), in `rows`.
    #[must_use]
    pub fn of(
        modules: &[ModuleView],
        density: Density,
        view: (u16, u16),
        rows: Option<&[Vec<String>]>,
    ) -> Self {
        Self {
            shapes: modules.iter().map(Shape::of).collect(),
            density,
            view,
            rows: rows.map(<[Vec<String>]>::to_vec),
        }
    }

    /// Whether a layout of `modules` at `density` for `view` in `rows`
    /// would be the one made for this key (checked without building
    /// anything).
    #[must_use]
    pub fn fits(
        &self,
        modules: &[ModuleView],
        density: Density,
        view: (u16, u16),
        rows: Option<&[Vec<String>]>,
    ) -> bool {
        self.view == view
            && self.density == density
            && self.rows.as_deref() == rows
            && self.shapes.len() == modules.len()
            && self
                .shapes
                .iter()
                .zip(modules)
                .all(|(shape, module)| shape.fits(module))
    }
}

/// A cable's path, kept from frame to frame: its cells and the box they
/// fill, so a cable wholly out of view is passed over at once.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hanging {
    /// Each cell the cable passes through, with its braille dot bits.
    pub cells: Vec<(Point, u8)>,
    /// The top-left of the box its cells fill.
    pub low: Point,
    /// The bottom-right of that box (inclusive).
    pub high: Point,
}

/// The path of a cable from `from` to `to` (see [`cable_path`]), with the
/// box it fills.
#[must_use]
pub fn hanging(from: Point, to: Point) -> Hanging {
    let cells: Vec<(Point, u8)> = cable_path(from, to).into_iter().collect();
    let low = Point {
        x: cells.iter().map(|(cell, _)| cell.x).min().unwrap_or(0),
        y: cells.iter().map(|(cell, _)| cell.y).min().unwrap_or(0),
    };
    let high = Point {
        x: cells.iter().map(|(cell, _)| cell.x).max().unwrap_or(0),
        y: cells.iter().map(|(cell, _)| cell.y).max().unwrap_or(0),
    };
    Hanging { cells, low, high }
}

/// The cells a cable from `from` to `to` passes through, as braille dots:
/// a curve that hangs between its ends like a real patch cable. Each item
/// is a cell and the dot bits set in it. The ends' own cells are left out
/// (the sockets are drawn there).
#[must_use]
pub fn cable_path(from: Point, to: Point) -> BTreeMap<Point, u8> {
    let dot = |point: Point| {
        (
            f64::from(point.x).mul_add(2.0, 1.0),
            f64::from(point.y).mul_add(4.0, 2.0),
        )
    };
    let (x0, y0) = dot(from);
    let (x2, y2) = dot(to);
    let distance = (x2 - x0).hypot(y2 - y0);
    // The slack: longer cables hang lower.
    let sag = distance.mul_add(0.25, 6.0);
    let (x1, y1) = (f64::midpoint(x0, x2), y0.max(y2) + sag);
    let steps = (distance * 2.0).ceil().max(8.0) as u32;
    let mut cells: BTreeMap<Point, u8> = BTreeMap::new();
    for step in 0..=steps {
        let t = f64::from(step) / f64::from(steps);
        let u = 1.0 - t;
        let x = (u * u).mul_add(x0, (2.0 * u * t).mul_add(x1, t * t * x2));
        let y = (u * u).mul_add(y0, (2.0 * u * t).mul_add(y1, t * t * y2));
        if x < 0.0 || y < 0.0 {
            continue;
        }
        let (dx, dy) = (x.round() as u32, y.round() as u32);
        let cell = Point {
            x: dx / 2,
            y: dy / 4,
        };
        if cell == from || cell == to {
            continue;
        }
        *cells.entry(cell).or_insert(0) |= dial::braille_bit(dx as usize, dy as usize);
    }
    cells
}

/// Where each cable runs, by cable number: the ends of every cable whose
/// jacks are both on the rack.
#[must_use]
pub fn cable_ends(
    rack: &Rack,
    modules: &[ModuleView],
    cables: &[CableRecord],
) -> Vec<(u32, Point, Point)> {
    let socket = |jack: &str, output: bool| {
        let (id, name) = jack.split_once('.')?;
        let index = modules.iter().position(|module| module.id == id)?;
        rack.plate(index)?
            .socket(name, output, &modules[index], rack.density)
    };
    cables
        .iter()
        .filter_map(|cable| {
            Some((
                cable.id,
                socket(&cable.from, true)?,
                socket(&cable.to, false)?,
            ))
        })
        .collect()
}

/// The pan that keeps it within the sheet: never past the far edge, and
/// never negative.
#[must_use]
pub fn clamp_pan(pan: Point, rack: &Rack, view_width: u16, view_height: u16) -> Point {
    Point {
        x: pan.x.min(rack.width.saturating_sub(u32::from(view_width))),
        y: pan
            .y
            .min(rack.height.saturating_sub(u32::from(view_height))),
    }
}

/// The pan, moved as little as possible, that shows the box at `at`
/// (`width` × `height`) in a view `view_width` × `view_height`.
#[must_use]
pub fn reveal(
    pan: Point,
    at: Point,
    width: u16,
    height: u16,
    view_width: u16,
    view_height: u16,
) -> Point {
    let axis = |pan: u32, at: u32, size: u16, view: u16| {
        let (size, view) = (u32::from(size), u32::from(view.max(1)));
        if at < pan {
            at
        } else if at + size > pan + view {
            (at + size).saturating_sub(view).min(at)
        } else {
            pan
        }
    };
    Point {
        x: axis(pan.x, at.x, width, view_width),
        y: axis(pan.y, at.y, height, view_height),
    }
}

/// The pan that puts the middle of the box at `at` (`width` × `height`)
/// in the middle of a view `view_width` × `view_height`, as near as the
/// sheet's top-left allows (clamp it to the sheet's far edges after).
#[must_use]
pub fn centred(at: Point, width: u16, height: u16, view_width: u16, view_height: u16) -> Point {
    let axis = |at: u32, size: u16, view: u16| {
        (at + u32::from(size / 2)).saturating_sub(u32::from(view / 2))
    };
    Point {
        x: axis(at.x, width, view_width),
        y: axis(at.y, height, view_height),
    }
}

/// `pan` moved by a drag of `dx`, `dy` cells (the sheet follows the mouse,
/// so the pan moves the other way).
#[must_use]
pub fn dragged(pan: Point, dx: i32, dy: i32) -> Point {
    let axis = |pan: u32, delta: i32| {
        if delta >= 0 {
            pan.saturating_sub(delta.unsigned_abs())
        } else {
            pan.saturating_add(delta.unsigned_abs())
        }
    };
    Point {
        x: axis(pan.x, dx),
        y: axis(pan.y, dy),
    }
}
