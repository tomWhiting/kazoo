//! The flames themselves: no box, no flat floor, many tongues, the branch
//! burning where it runs, and a moment left behind dying down where it was.

use super::field::Field;
use super::fuel::Fuel;
use super::place::shape_of;
use super::shape::Shape;
use super::*;
use crate::growth::Growth;
use crate::history::History;
use crate::render::{Level, Scene, View};
use crate::test_support::grown;

/// A field at `level` burning along a branch through its moment running
/// `slope` dots down for every dot across.
fn burning_on(level: Level, slope: f32) -> Field {
    let shape = shape_of(level, 4.0, 200, 60);
    let mut field = Field::new(shape);
    field.burn_on(&branch(&shape, slope));
    field
}

fn branch(shape: &Shape, slope: f32) -> Fuel {
    let middle = ((shape.w / 2) as f32, shape.base as f32);
    let step = 4.0 / slope.mul_add(slope, 1.0).sqrt();
    let point = |k: f32| {
        (
            k.mul_add(step, middle.0),
            (k * step).mul_add(slope, middle.1),
        )
    };
    let before: Vec<_> = (1..=8).map(|k| point(-(k as f32))).collect();
    let after: Vec<_> = (1..=8).map(|k| point(k as f32)).collect();
    Fuel::line(&before, middle, &after).expect("the moment is a number")
}

/// The heat drawn at dot `(x, y)`.
fn heat(field: &Field, x: usize, y: usize) -> f32 {
    field.at(
        i32::try_from(x).unwrap_or(i32::MAX),
        i32::try_from(y).unwrap_or(i32::MAX),
    )
}

fn burn(field: &mut Field, seconds: f32) {
    for _ in 0..(seconds * 120.0) as usize {
        field.step(1.0 / 120.0);
    }
}

#[test]
fn nothing_burns_near_the_fields_edges_so_it_has_no_box() {
    for level in [Level::Far, Level::Near, Level::Close, Level::Every] {
        let mut field = burning_on(level, 0.0);
        let Shape { w, h, .. } = field.shape();
        // The outer quarter of the band the heat fades across: at most
        // about a sixth as hot as the flames, whatever burns inside it.
        let side = ((w as f32 * 0.12).max(3.0) * 0.25).ceil() as usize;
        let top = ((h as f32 * 0.15).max(4.0) * 0.25).ceil() as usize;
        let mut hottest = 0.0f32;
        for _ in 0..600 {
            field.step(1.0 / 120.0);
            for y in 0..h {
                for x in (0..side).chain(w - side..w) {
                    let v = heat(&field, x, y);
                    assert!(v < 0.17, "{level:?}: heat {v} at the side, {x}, {y}");
                }
            }
            for y in 0..top {
                for x in 0..w {
                    let v = heat(&field, x, y);
                    assert!(v < 0.17, "{level:?}: heat {v} at the top, {x}, {y}");
                }
            }
            hottest = field.seen().iter().fold(hottest, |a, &b| a.max(b));
        }
        assert!(
            hottest > 0.6,
            "{level:?}: the control: it burned, {hottest}"
        );
    }
}

#[test]
fn the_branch_burns_in_ragged_tongues_never_a_flat_floor() {
    for level in [Level::Close, Level::Every] {
        let mut field = burning_on(level, 0.0);
        burn(&mut field, 2.0);
        let shape = field.shape();
        let middle = (shape.w / 2) as f32;
        // Along the branch, either side of the moment's own core: the
        // tongues' roots stand apart, so every frame the row falls cool
        // between them. Fed evenly, it never falls below about 0.55.
        let span = shape.reach * 0.7;
        let row: Vec<usize> = ((middle - span) as usize..=(middle + span) as usize)
            .filter(|&x| (x as f32 - middle).abs() > shape.thick * 1.5)
            .collect();
        let mut hottest = 0.0f32;
        for _ in 0..240 {
            field.step(1.0 / 120.0);
            for y in shape.base - 2..=shape.base {
                let (lo, hi) = row.iter().fold((1.0f32, 0.0f32), |(lo, hi), &x| {
                    let v = heat(&field, x, y);
                    (lo.min(v), hi.max(v))
                });
                assert!(lo < 0.48, "{level:?}: row {y} is a flat floor: {lo}..{hi}");
                hottest = hottest.max(hi);
            }
        }
        assert!(
            hottest > 0.7,
            "{level:?}: the control: the tongues burned, {hottest}"
        );
    }
}

/// Runs of hot dots across row `y`: the tongues crossing it.
fn tongues(field: &Field, y: usize) -> usize {
    let mut runs = 0;
    let mut inside = false;
    for x in 0..field.shape().w {
        let v = heat(field, x, y);
        if !inside && v > 0.45 {
            runs += 1;
            inside = true;
        } else if inside && v < 0.2 {
            inside = false;
        }
    }
    runs
}

#[test]
fn close_in_there_are_many_tongues_not_one_plume() {
    // Close in the fire is smaller, so fewer tongues stand apart at once.
    for (level, least) in [(Level::Close, 60), (Level::Every, 120)] {
        let mut field = burning_on(level, 0.0);
        burn(&mut field, 2.0);
        let shape = field.shape();
        let y = shape.base - (shape.tall * 0.3) as usize;
        let mut most = 0;
        let mut frames_with_several = 0;
        for _ in 0..240 {
            field.step(1.0 / 120.0);
            let n = tongues(&field, y);
            most = most.max(n);
            frames_with_several += usize::from(n >= 2);
        }
        assert!(most >= 3, "{level:?}: at most {most} tongues");
        assert!(
            frames_with_several > least,
            "{level:?}: one plume mostly, {frames_with_several} of 240"
        );
    }
}

#[test]
fn a_branch_running_down_burns_below_the_moment_too() {
    let mut field = burning_on(Level::Every, 1.0);
    burn(&mut field, 2.0);
    let shape = field.shape();
    // Half way along the branch's burning stretch, down and to the right.
    let along = shape.reach * 0.4 / 2.0f32.sqrt();
    let (x, y) = ((shape.w / 2) as f32 + along, shape.base as f32 + along);
    let mut hottest = 0.0f32;
    for _ in 0..60 {
        field.step(1.0 / 120.0);
        hottest = hottest.max(field.at(x as i32, y as i32));
    }
    assert!(
        hottest > 0.3,
        "the branch below the moment is cold: {hottest}"
    );
}

fn tree() -> (History, Growth) {
    let history = grown(200, &[(0, 50, 60), (0, 120, 40)]);
    let growth = Growth::grow(&history);
    (history, growth)
}

fn total(field: &Field) -> f32 {
    field.seen().iter().sum()
}

/// The tree, the wall's moment and a view on it, to draw a blaze into.
struct Stage {
    history: History,
    growth: Growth,
    view: View,
    buf: ratatui::buffer::Buffer,
}

impl Stage {
    fn new() -> (Self, usize) {
        let (history, growth) = tree();
        let first = history.here.expect("the wall has a place");
        let at = growth.at[first].expect("grown");
        let area = ratatui::layout::Rect::new(0, 0, 160, 50);
        let stage = Self {
            history,
            growth,
            view: View {
                centre: at,
                scale: 4.0,
            },
            buf: ratatui::buffer::Buffer::empty(area),
        };
        (stage, first)
    }

    fn draw(&mut self, blaze: &mut Blaze) {
        let scene = Scene {
            history: &self.history,
            growth: &self.growth,
            gestures: &[],
            days: &[],
            line: &[],
            selected: None,
            now: 0,
        };
        let area = self.buf.area;
        blaze.draw(&scene, &self.view, area, &mut self.buf);
    }

    /// Burn on for `seconds`, a frame drawn each step.
    fn burn(&mut self, blaze: &mut Blaze, seconds: f32) {
        for _ in 0..(seconds * 120.0) as usize {
            blaze.burn(1.0 / 120.0);
            self.draw(blaze);
        }
    }

    /// Put the wall at `here`, and draw the frame that sees it.
    fn go(&mut self, blaze: &mut Blaze, here: Option<usize>) {
        self.history.here = here;
        self.draw(blaze);
    }
}

#[test]
fn a_moment_left_behind_dies_down_where_it_was_as_the_next_catches() {
    let (mut stage, first) = Stage::new();
    let mut blaze = Blaze::default();
    stage.draw(&mut blaze);
    stage.burn(&mut blaze, 2.0);
    let was = blaze.placed().expect("in view");
    let burning = total(blaze.field());
    assert!(
        burning > 10.0,
        "the control: the first moment burned, {burning}"
    );
    // The wall goes back along the trunk, well away on screen.
    stage.go(&mut blaze, Some(first - 40));
    let behind = blaze.left_behind().expect("the first moment dies down");
    assert!(
        (total(behind) - burning).abs() < 1.0,
        "it did not jump: it is the same fire"
    );
    assert!(total(blaze.field()) < 1.0, "the next moment starts cold");
    assert_ne!(blaze.placed().expect("in view").anchor, was.anchor);
    // Each quarter second the old fire is smaller, until it is gone; the
    // new one grows. A flicker either way is not a flare.
    let (mut old, mut new) = (burning, 0.0f32);
    for quarter in 0..24 {
        stage.burn(&mut blaze, 0.25);
        let now = blaze.left_behind().map_or(0.0, total);
        assert!(
            now <= old.mul_add(1.05, 0.5),
            "quarter {quarter}: the old fire flared, {old} -> {now}"
        );
        old = now;
        let grown = total(blaze.field());
        if quarter < 4 {
            assert!(
                grown >= new.mul_add(0.95, -0.5),
                "quarter {quarter}: the new fire faltered, {new} -> {grown}"
            );
        }
        new = grown;
    }
    assert!(
        blaze.left_behind().is_none(),
        "the old fire never went out: {old}"
    );
    assert!(new > burning * 0.5, "the new fire never caught: {new}");
}

#[test]
fn every_moment_left_dies_down_on_its_own_until_all_are_out() {
    let (mut stage, first) = Stage::new();
    let mut blaze = Blaze::default();
    stage.draw(&mut blaze);
    stage.burn(&mut blaze, 2.0);
    // A, then B, then C, a moment apart: A and B both die down.
    stage.go(&mut blaze, Some(first - 20));
    stage.burn(&mut blaze, 0.1);
    stage.go(&mut blaze, Some(first - 40));
    assert_eq!(blaze.dying(), 2, "A and B both die down");
    let b = total(blaze.left_behind().expect("B dies down"));
    assert!(b > 0.0, "the control: B had caught, {b}");
    stage.burn(&mut blaze, 6.0);
    assert_eq!(blaze.dying(), 0, "every fire left went out");
    assert!(total(blaze.field()) > 10.0, "C caught");
}

#[test]
fn going_back_to_a_moment_takes_its_fire_up_where_it_was() {
    let (mut stage, first) = Stage::new();
    let mut blaze = Blaze::default();
    stage.draw(&mut blaze);
    stage.burn(&mut blaze, 2.0);
    // A to B, and straight back to A while it still burns.
    stage.go(&mut blaze, Some(first - 40));
    stage.burn(&mut blaze, 0.1);
    let a = total(blaze.left_behind().expect("A dies down"));
    stage.go(&mut blaze, Some(first));
    assert_eq!(blaze.dying(), 1, "B dies down, A burns again");
    let back = total(blaze.field());
    assert!(
        a > 10.0 && (back - a).abs() < 1.0,
        "A takes up where it was, not from cold: {a} -> {back}"
    );
    stage.burn(&mut blaze, 2.0);
    assert!(total(blaze.field()) > 10.0, "A burns on");
    assert_eq!(blaze.dying(), 0, "B went out");
}

#[test]
fn a_wall_at_no_moment_lets_its_fire_die_down() {
    let (mut stage, _) = Stage::new();
    let mut blaze = Blaze::default();
    stage.draw(&mut blaze);
    stage.burn(&mut blaze, 2.0);
    let burning = total(blaze.field());
    assert!(burning > 10.0, "the control: it burned, {burning}");
    stage.go(&mut blaze, None);
    assert_eq!(blaze.dying(), 1, "the moment left dies down");
    assert!(blaze.placed().is_none(), "nothing burns for no moment");
    stage.burn(&mut blaze, 6.0);
    assert_eq!(blaze.dying(), 0, "and goes out");
    assert!(!blaze.live(), "with nothing burning, no frames are drawn");
}

#[test]
fn a_wall_growing_on_from_its_moment_carries_its_fire_on() {
    let (mut stage, _) = Stage::new();
    let trunk = stage.history.limbs[0].moments.clone();
    let mut blaze = Blaze::default();
    stage.go(&mut blaze, Some(trunk[150]));
    // A change every half second, each grown from the last: the same fire
    // burns on, catching as it would on one moment.
    for step in 1..=5 {
        stage.burn(&mut blaze, 0.5);
        let before = blaze.caught();
        stage.go(&mut blaze, Some(trunk[150 + step]));
        assert_eq!(
            blaze.dying(),
            0,
            "step {step}: a moment grown on is no jump"
        );
        assert!(
            blaze.caught() >= before,
            "step {step}: it caught again from cold, {before} -> {}",
            blaze.caught()
        );
    }
    stage.burn(&mut blaze, 0.5);
    assert!(
        blaze.caught() >= 1.0,
        "never a full blaze: {}",
        blaze.caught()
    );
    let hot = total(blaze.field());
    assert!(hot > 10.0, "the field went cold: {hot}");
}

#[test]
fn a_go_far_along_the_same_branch_still_sinks_and_catches() {
    let (mut stage, _) = Stage::new();
    let trunk = stage.history.limbs[0].moments.clone();
    // Every moment a dot: a go 40 changes on is a jump across the screen.
    stage.view = View {
        centre: stage.growth.at[trunk[170]].expect("grown"),
        scale: 8.0,
    };
    let mut blaze = Blaze::default();
    stage.go(&mut blaze, Some(trunk[150]));
    stage.burn(&mut blaze, 2.0);
    assert!(blaze.caught() > 0.9, "the control: it caught");
    stage.go(&mut blaze, Some(trunk[190]));
    assert_eq!(blaze.dying(), 1, "the moment left dies down where it was");
    assert!(blaze.caught() <= 0.0, "the new moment catches from a spark");
}

#[test]
fn a_jump_to_another_branch_still_sinks_the_old_fire_and_catches_the_new() {
    let (mut stage, _) = Stage::new();
    let trunk = stage.history.limbs[0].moments.clone();
    let bud = stage.history.limbs[1].moments[10];
    let mut blaze = Blaze::default();
    stage.go(&mut blaze, Some(trunk[150]));
    stage.burn(&mut blaze, 2.0);
    assert!(total(blaze.field()) > 10.0, "the control: it burned");
    stage.go(&mut blaze, Some(bud));
    assert_eq!(blaze.dying(), 1, "the moment left dies down");
    assert!(blaze.caught() <= 0.0, "the new moment catches from a spark");
    assert!(total(blaze.field()) < 1.0, "the new moment starts cold");
}

#[test]
fn spent_fires_are_not_kept_beyond_one() {
    let (mut stage, _) = Stage::new();
    let trunk = stage.history.limbs[0].moments.clone();
    let mut blaze = Blaze::default();
    stage.go(&mut blaze, Some(trunk[150]));
    stage.burn(&mut blaze, 1.0);
    // Six jumps back along the trunk, each caught a little: several fires
    // die down at once.
    for k in 1..=6 {
        stage.go(&mut blaze, Some(trunk[150 - 10 * k]));
        stage.burn(&mut blaze, 0.3);
    }
    assert!(
        blaze.dying() >= 3,
        "the control: they die down together, {}",
        blaze.dying()
    );
    stage.burn(&mut blaze, 6.0);
    assert_eq!(blaze.dying(), 0, "every fire left went out");
    assert!(blaze.spares() <= 1, "{} spent fires kept", blaze.spares());
}

/// The field as braille, for looking at.
fn picture(field: &Field) -> String {
    const DOT: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    let Shape { w, h, .. } = field.shape();
    let mut out = String::new();
    for row in 0..h.div_ceil(4) {
        for col in 0..w.div_ceil(2) {
            let mut bits = 0;
            for (dy, line) in DOT.iter().enumerate() {
                for (dx, bit) in line.iter().enumerate() {
                    let v = heat(field, col * 2 + dx, row * 4 + dy);
                    if v > 0.08 + 0.85 * ((dy * 2 + dx) as f32 + 0.5) / 8.0 {
                        bits |= bit;
                    }
                }
            }
            out.push(char::from_u32(0x2800 | bits).unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

/// Prints frames to look at: `cargo nextest run -p kazoo-tree
/// --run-ignored only show_frames --no-capture`.
#[test]
#[ignore = "prints frames for a person to look at"]
fn show_frames() {
    let mut out = Vec::new();
    for (level, slope) in [
        (Level::Near, 0.3),
        (Level::Close, -0.4),
        (Level::Every, 0.15),
        (Level::Every, 1.2),
    ] {
        let mut field = burning_on(level, slope);
        burn(&mut field, 3.0);
        for k in 0..2 {
            burn(&mut field, 0.15);
            out.push(format!(
                "{level:?} slope {slope} frame {k}\n{}",
                picture(&field)
            ));
        }
    }
    println!("{}", out.join("\n"));
}
