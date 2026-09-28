//! The rack view's layout maths: faceplate sizes, wrapping into rows, what
//! is under the mouse, where cables run, and panning.

use kazoo_wall::protocol::ModuleView;

use super::super::rack::geometry::{
    self, Density, GAP_X, Hit, JACK_H, JACK_W, KNOB_H, KNOB_ROWS, KNOB_W, METER_W, MIN_INNER,
    Point, Rack, TITLE_H,
};
use super::{cable, module, snapshot};

fn modules() -> Vec<ModuleView> {
    snapshot().modules
}

#[test]
fn faceplates_are_as_wide_as_their_knobs_and_as_tall_as_their_jacks_need() {
    let filter = module("vcf1", "vcf", None);
    let knob_columns = u16::try_from(filter.knobs.len())
        .expect("few knobs")
        .div_ceil(KNOB_ROWS);
    let inner = (knob_columns * KNOB_W).max(MIN_INNER);
    let jack_columns = inner / JACK_W;
    let jack_rows = u16::try_from(filter.inputs.len())
        .expect("few inputs")
        .div_ceil(jack_columns)
        + u16::try_from(filter.outputs.len())
            .expect("few outputs")
            .div_ceil(jack_columns);
    assert_eq!(
        geometry::plate_size(&filter, Density::Full),
        (
            inner + 2,
            2 + TITLE_H + KNOB_ROWS * KNOB_H + jack_rows * JACK_H
        )
    );

    // A sequencer's many knobs make a wide faceplate.
    let sequencer = module("seq1", "seq", None);
    let (width, _) = geometry::plate_size(&sequencer, Density::Full);
    let columns = u16::try_from(sequencer.knobs.len())
        .expect("few knobs")
        .div_ceil(KNOB_ROWS);
    assert_eq!(width, columns * KNOB_W + 2);

    // An out has room for its meter.
    let out = module("out1", "out", None);
    let (width, _) = geometry::plate_size(&out, Density::Full);
    let knob_columns = u16::try_from(out.knobs.len())
        .expect("few knobs")
        .div_ceil(KNOB_ROWS);
    assert_eq!(width, (knob_columns * KNOB_W + METER_W).max(MIN_INNER) + 2);
}

#[test]
fn the_rack_wraps_into_the_rows_that_fit_and_every_plate_fills_its_row() {
    let modules = modules();
    let tallest = modules
        .iter()
        .map(|module| geometry::plate_size(module, Density::Full).1)
        .max()
        .expect("modules");
    // One row when only one fits.
    let one = geometry::rack(&modules, Density::Full, (400, tallest), None);
    assert!(one.plates.iter().all(|plate| plate.at.y == 0));
    assert!(one.plates.iter().all(|plate| plate.height == tallest));
    let mut x = u32::from(GAP_X);
    for plate in &one.plates {
        assert_eq!(plate.at.x, x, "plates sit side by side, a column apart");
        x += u32::from(plate.width + GAP_X);
    }
    assert_eq!(one.width, x);
    assert_eq!(one.height, u32::from(tallest));

    // Two rows when two fit: about the same width each, in order.
    let two = geometry::rack(&modules, Density::Full, (400, tallest * 2 + 1), None);
    let first_row = modules[..2]
        .iter()
        .map(|module| geometry::plate_size(module, Density::Full).1)
        .max()
        .expect("modules");
    let second = u32::from(first_row) + 1;
    let rows: Vec<u32> = two.plates.iter().map(|plate| plate.at.y).collect();
    assert_eq!(rows, vec![0, 0, second, second]);
    assert!(two.width < one.width);

    // Nothing to lay out is an empty rack.
    assert_eq!(
        geometry::rack(&[], Density::Full, (400, 40), None),
        Rack::default()
    );
    // A sliver of a screen still lays out, in one row.
    let sliver = geometry::rack(&modules, Density::Full, (400, 1), None);
    assert!(sliver.plates.iter().all(|plate| plate.at.y == 0));
}

#[test]
fn the_mouse_finds_knobs_jacks_plates_and_the_bare_rack() {
    let modules = modules();
    let rack = geometry::rack(&modules, Density::Full, (400, 60), None);
    let plate = rack.plate(2).expect("the filter's faceplate");
    let cutoff = plate.knobs[0];
    assert_eq!(
        rack.hit(Point {
            x: cutoff.at.x + 3,
            y: cutoff.at.y + 1
        }),
        Hit::Knob { module: 2, knob: 0 }
    );
    let last_cell = Point {
        x: cutoff.at.x + u32::from(KNOB_W) - 1,
        y: cutoff.at.y + u32::from(KNOB_H) - 1,
    };
    assert_eq!(rack.hit(last_cell), Hit::Knob { module: 2, knob: 0 });
    let input = plate
        .jacks
        .iter()
        .find(|jack| !jack.output)
        .expect("the filter has an input");
    assert_eq!(
        rack.hit(Point {
            x: input.at.x + 2,
            y: input.at.y + 1
        }),
        Hit::Jack {
            module: 2,
            name: input.name.clone(),
            output: false
        }
    );
    // The title bar, top edge and name, is where it is picked up.
    for y in [plate.at.y, plate.at.y + 1, plate.at.y + u32::from(TITLE_H)] {
        assert_eq!(
            rack.hit(Point {
                x: plate.at.x + 2,
                y
            }),
            Hit::Title { module: 2 }
        );
    }
    // Between the knobs and the jacks is the faceplate itself.
    let below = plate
        .knobs
        .iter()
        .map(|knob| knob.at.y)
        .max()
        .expect("knobs")
        + u32::from(KNOB_H);
    assert_eq!(
        rack.hit(Point {
            x: plate.at.x + u32::from(plate.width) - 2,
            y: below.min(input.at.y - 1)
        }),
        Hit::Plate { module: 2 }
    );
    // Left of the first faceplate, and far beyond the last, is bare rack.
    assert_eq!(rack.hit(Point { x: 0, y: 0 }), Hit::Background);
    assert_eq!(
        rack.hit(Point {
            x: rack.width + 10,
            y: 0
        }),
        Hit::Background
    );
}

#[test]
fn cables_run_between_their_sockets_and_hang_below_them() {
    let snapshot = snapshot();
    let rack = geometry::rack(&snapshot.modules, Density::Full, (400, 60), None);
    let ends = geometry::cable_ends(&rack, &snapshot.modules, &snapshot.cables);
    assert_eq!(ends.len(), 3);
    let &(id, from, to) = ends.iter().find(|(id, ..)| *id == 2).expect("cable 2");
    assert_eq!(id, 2);
    // Cable 2 plugs into the filter's cutoff knob: it ends at the knob.
    let filter = rack.plate(2).expect("the filter");
    assert_eq!(to, filter.knobs[0].at);
    // It comes from the LFO's output socket.
    let lfo = rack.plate(1).expect("the lfo");
    let out = lfo
        .jacks
        .iter()
        .find(|jack| jack.output)
        .expect("an output");
    assert_eq!(
        from,
        Point {
            x: out.at.x + 2,
            y: out.at.y
        }
    );

    let path = geometry::cable_path(from, to);
    assert!(
        !path.contains_key(&from) && !path.contains_key(&to),
        "the sockets show"
    );
    assert!(path.values().all(|&bits| bits != 0));
    let lowest = path.keys().map(|cell| cell.y).max().expect("a path");
    assert!(lowest >= from.y.max(to.y));
    // Between sockets on one level it hangs below them.
    let level = geometry::cable_path(Point { x: 10, y: 5 }, Point { x: 40, y: 5 });
    assert!(level.keys().map(|cell| cell.y).max().expect("a path") > 6);
    // It is unbroken: every cell touches another.
    for cell in path.keys() {
        let touching = path.keys().any(|other| {
            other != cell && other.x.abs_diff(cell.x) <= 1 && other.y.abs_diff(cell.y) <= 1
        });
        assert!(touching, "{cell:?} stands alone");
    }

    // A cable to a module that is not there has no ends.
    let stray = vec![cable(9, "vco9.out", "vcf1.in", 1.0)];
    assert!(geometry::cable_ends(&rack, &snapshot.modules, &stray).is_empty());
}

#[test]
fn panning_stays_on_the_rack_and_follows_what_is_selected() {
    let rack = Rack {
        plates: Vec::new(),
        width: 300,
        height: 60,
        density: Density::Full,
        bands: Vec::new(),
        blanks: Vec::new(),
    };
    assert_eq!(
        geometry::clamp_pan(Point { x: 500, y: 90 }, &rack, 100, 40),
        Point { x: 200, y: 20 }
    );
    assert_eq!(
        geometry::clamp_pan(Point { x: 5, y: 5 }, &rack, 400, 80),
        Point { x: 0, y: 0 }
    );
    // Reveal moves as little as it can.
    let pan = Point { x: 0, y: 0 };
    assert_eq!(
        geometry::reveal(pan, Point { x: 150, y: 10 }, 20, 5, 100, 40),
        Point { x: 70, y: 0 }
    );
    assert_eq!(
        geometry::reveal(
            Point { x: 120, y: 0 },
            Point { x: 100, y: 0 },
            10,
            5,
            100,
            40
        ),
        Point { x: 100, y: 0 }
    );
    assert_eq!(
        geometry::reveal(pan, Point { x: 10, y: 10 }, 10, 5, 100, 40),
        pan
    );
    // Dragging the rack moves the sheet with the mouse; never below zero.
    assert_eq!(
        geometry::dragged(Point { x: 50, y: 10 }, -20, 5),
        Point { x: 70, y: 5 }
    );
    assert_eq!(
        geometry::dragged(Point { x: 5, y: 5 }, 30, 30),
        Point { x: 0, y: 0 }
    );
}

#[test]
fn a_layout_key_changes_with_the_wall_s_shape_alone() {
    let modules = modules();
    let key = geometry::LayoutKey::of(&modules, Density::Full, (160, 40), None);
    assert!(key.fits(&modules, Density::Full, (160, 40), None));
    assert!(
        !key.fits(&modules, Density::Full, (160, 41), None),
        "a new height lays out again"
    );
    // Knob values and names move nothing.
    let mut turned = modules.clone();
    turned[2].knobs[0].value = 1234.0;
    turned[2].name = Some("renamed".to_string());
    assert!(key.fits(&turned, Density::Full, (160, 40), None));
    // Another module, one fewer, or a new jack does.
    let mut more = modules.clone();
    more.push(module("vco2", "vco", None));
    assert!(!key.fits(&more, Density::Full, (160, 40), None));
    assert!(!key.fits(&modules[1..], Density::Full, (160, 40), None));
    let mut jacks = modules.clone();
    jacks[0].inputs.push("sync".to_string());
    assert!(!key.fits(&jacks, Density::Full, (160, 40), None));
    let mut swapped = modules;
    swapped.swap(0, 2);
    assert!(!key.fits(&swapped, Density::Full, (160, 40), None));
}

#[test]
fn a_hanging_cable_knows_the_box_it_fills() {
    let from = Point { x: 10, y: 5 };
    let to = Point { x: 40, y: 8 };
    let path = geometry::hanging(from, to);
    let cells = geometry::cable_path(from, to);
    assert_eq!(path.cells, cells.into_iter().collect::<Vec<_>>());
    for (cell, _) in &path.cells {
        assert!(cell.x >= path.low.x && cell.x <= path.high.x);
        assert!(cell.y >= path.low.y && cell.y <= path.high.y);
    }
    assert!(path.cells.iter().any(|(cell, _)| cell.x == path.low.x));
    assert!(path.cells.iter().any(|(cell, _)| cell.y == path.high.y));
}

#[test]
fn the_faceplate_below_is_the_nearest_on_the_next_row() {
    let modules: Vec<ModuleView> = (1..=6)
        .map(|n| module(&format!("vco{n}"), "vco", None))
        .collect();
    let tallest = geometry::plate_size(&modules[0], Density::Full).1;
    let rack = geometry::rack(&modules, Density::Full, (400, tallest * 2 + 1), None);
    let top: Vec<usize> = rack
        .plates
        .iter()
        .filter(|plate| plate.at.y == 0)
        .map(|plate| plate.module)
        .collect();
    assert_eq!(top, vec![0, 1, 2], "three to a row");
    assert_eq!(rack.beside(0, true), Some(3));
    assert_eq!(rack.beside(2, true), Some(5));
    assert_eq!(rack.beside(4, false), Some(1));
    assert_eq!(rack.beside(1, false), None, "nothing above the top row");
    assert_eq!(rack.beside(4, true), None, "nothing below the bottom row");
    assert_eq!(rack.beside(9, true), None, "no such module");
}

#[test]
fn across_goes_by_what_is_beside_it_on_screen() {
    let modules: Vec<ModuleView> = (1..=6)
        .map(|n| module(&format!("vco{n}"), "vco", None))
        .collect();
    let tallest = geometry::plate_size(&modules[0], Density::Full).1;
    let mut rack = geometry::rack(&modules, Density::Full, (400, tallest * 2 + 1), None);
    // Along a row, then on to the next.
    assert_eq!(rack.across(0, true), Some(1));
    assert_eq!(rack.across(2, true), Some(3));
    assert_eq!(rack.across(3, false), Some(2));
    // The ends stay put; a module with no faceplate has nowhere to go.
    assert_eq!(rack.across(0, false), Some(0));
    assert_eq!(rack.across(5, true), Some(5));
    assert_eq!(rack.across(9, true), None);
    // By place on screen, not by number: swap two faceplates' places.
    let (first, last) = (rack.plates[0].at, rack.plates[5].at);
    rack.plates[0].at = last;
    rack.plates[5].at = first;
    assert_eq!(rack.across(5, true), Some(1));
    assert_eq!(rack.across(4, true), Some(0));
    assert_eq!(rack.across(0, true), Some(0));
}

#[test]
fn centring_puts_the_middle_of_a_box_in_the_middle_of_the_view() {
    assert_eq!(
        geometry::centred(Point { x: 200, y: 30 }, 20, 10, 100, 40),
        Point { x: 160, y: 15 }
    );
    // Near the sheet's start it goes no further than the start.
    assert_eq!(
        geometry::centred(Point { x: 10, y: 2 }, 20, 10, 100, 40),
        Point { x: 0, y: 0 }
    );
}

#[test]
fn compact_faceplates_are_smaller_and_overview_blocks_smaller_still() {
    for module in modules() {
        let full = geometry::plate_size(&module, Density::Full);
        let compact = geometry::plate_size(&module, Density::Compact);
        let overview = geometry::plate_size(&module, Density::Overview);
        assert!(compact.0 <= full.0, "{}: {compact:?} {full:?}", module.id);
        assert!(compact.1 < full.1, "{}: {compact:?} {full:?}", module.id);
        assert!(compact.1 <= 19, "{}: {compact:?}", module.id);
        assert_eq!(overview.1, geometry::OVERVIEW_H);
        assert!(overview.0 >= geometry::OVERVIEW_INNER + 2);
        assert!(overview.0 <= compact.0.max(geometry::OVERVIEW_INNER + 2));
    }
    // A compact knob is 6 wide and 4 tall: a 4 × 2 dial, its name, its
    // value.
    let scale = Density::Compact.scale();
    assert_eq!((scale.knob_w, scale.knob_h), (6, 4));
    let sequencer = module("seq1", "seq", None);
    let full = geometry::plate_size(&sequencer, Density::Full).0;
    let compact = geometry::plate_size(&sequencer, Density::Compact).0;
    assert!(
        f64::from(compact) <= f64::from(full) * 0.8,
        "{compact} vs {full}"
    );
    assert_eq!(Density::Full.next(), Density::Compact);
    assert_eq!(Density::Compact.next(), Density::Overview);
    assert_eq!(Density::Overview.next(), Density::Full);
}

#[test]
fn the_mouse_finds_knobs_and_jacks_at_every_density() {
    let modules = modules();
    let compact = geometry::rack(&modules, Density::Compact, (400, 60), None);
    assert_eq!(compact.density, Density::Compact);
    let filter = compact.plate(2).expect("the filter");
    let scale = Density::Compact.scale();
    let cutoff = filter.knobs[0];
    assert_eq!(
        compact.hit(Point {
            x: cutoff.at.x + u32::from(scale.knob_w) - 1,
            y: cutoff.at.y + u32::from(scale.knob_h) - 1,
        }),
        Hit::Knob { module: 2, knob: 0 }
    );
    let second = filter.knobs[1];
    assert_eq!(second.at.y, cutoff.at.y + u32::from(scale.knob_h));
    // A compact jack is one line: its socket, then its name.
    let input = filter
        .jacks
        .iter()
        .find(|jack| !jack.output)
        .expect("an input");
    let hit = Hit::Jack {
        module: 2,
        name: input.name.clone(),
        output: false,
    };
    assert_eq!(compact.hit(input.at), hit);
    assert_eq!(
        compact.hit(Point {
            x: input.at.x + u32::from(scale.jack_w) - 1,
            ..input.at
        }),
        hit
    );
    assert_ne!(
        compact.hit(Point {
            y: input.at.y + 1,
            ..input.at
        }),
        hit
    );

    // The overview: no knobs to hit, a socket per jack, and every knob's
    // cable ends at the one socket they share.
    let overview = geometry::rack(&modules, Density::Overview, (400, 60), None);
    let filter = overview.plate(2).expect("the filter");
    assert!(
        filter
            .knobs
            .iter()
            .all(|knob| knob.at == filter.knobs[0].at)
    );
    assert!(matches!(
        overview.hit(filter.knobs[0].at),
        Hit::Plate { module: 2 }
    ));
    for jack in &filter.jacks {
        assert_eq!(jack.at.y, filter.at.y + u32::from(geometry::OVERVIEW_H) - 2);
        assert_eq!(
            overview.hit(jack.at),
            Hit::Jack {
                module: 2,
                name: jack.name.clone(),
                output: jack.output
            }
        );
    }
    let snapshot = snapshot();
    let ends = geometry::cable_ends(&overview, &snapshot.modules, &snapshot.cables);
    let &(_, _, to) = ends.iter().find(|(id, ..)| *id == 2).expect("cable 2");
    assert_eq!(to, filter.knobs[0].at, "into the cutoff: the knobs' socket");
    let &(_, _, to) = ends.iter().find(|(id, ..)| *id == 1).expect("cable 1");
    let input = filter
        .jacks
        .iter()
        .find(|jack| jack.name == "in")
        .expect("the filter's input");
    assert_eq!(to, input.at);
    // Small blocks fill the width before they start another row.
    let wide = geometry::rack(&modules, Density::Overview, (400, 60), None);
    assert!(wide.plates.iter().all(|plate| plate.at.y == 0));
    let narrow = geometry::rack(&modules, Density::Overview, (30, 60), None);
    assert!(narrow.plates.iter().any(|plate| plate.at.y > 0));
}
