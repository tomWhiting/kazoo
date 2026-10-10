//! A scope's screen in braille: each cell 2 × 4 dots, the trace of input
//! a over input b over a faint graticule (the zero line, and the trigger
//! level in `trigger` mode), as the scope's latest sweep has it.
//!
//! Each point of the sweep is a stroke from its lowest to its highest,
//! joined to the point before, so a fast wave on a long span shows its
//! whole swing as a band. The points come oldest first: a sweep is drawn
//! from the left (one under way stops where it has got to, over what is
//! left of the last), and a rolling screen right-aligned, filling from the
//! right. In `xy` mode input a goes across and b up, on square axes so a
//! circle is round. Values beyond the screen are drawn on its edge.

use kazoo_wall::format::{hertz, number, seconds};
use kazoo_wall::protocol::ScopeReading;

use super::dial::{braille, braille_bit};

/// What a cell of the screen shows most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// Nothing.
    Blank,
    /// The graticule.
    Grid,
    /// Input a's trace.
    A,
    /// Input b's trace (and nothing of a's).
    B,
}

/// One cell: its braille character (U+2800 for none, so no cable is drawn
/// across the screen) and what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The braille character.
    pub glyph: char,
    /// What it shows, for its colour.
    pub ink: Ink,
}

/// Dots, by layer.
struct Dots {
    cols: usize,
    rows: usize,
    layers: [Vec<u8>; 3],
}

impl Dots {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            layers: [
                vec![0; cols * rows],
                vec![0; cols * rows],
                vec![0; cols * rows],
            ],
        }
    }

    const fn width(&self) -> usize {
        self.cols * 2
    }

    const fn height(&self) -> usize {
        self.rows * 4
    }

    /// Set dot `(x, y)` (rounded) on `layer`, if it is on the screen.
    fn set(&mut self, layer: usize, x: f64, y: f64) {
        let (x, y) = (x.round(), y.round());
        if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.width() || y >= self.height() {
            return;
        }
        self.layers[layer][y / 4 * self.cols + x / 2] |= braille_bit(x, y);
    }

    /// A line of dots from `from` to `to` on `layer`.
    fn line(&mut self, layer: usize, from: (f64, f64), to: (f64, f64)) {
        let steps = (to.0 - from.0).abs().max((to.1 - from.1).abs()).ceil();
        let steps = if steps.is_finite() {
            steps.clamp(1.0, 4_096.0)
        } else {
            1.0
        };
        // At most 4096: exact.
        let count = steps as u32;
        for step in 0..=count {
            let t = f64::from(step) / steps;
            self.set(
                layer,
                (to.0 - from.0).mul_add(t, from.0),
                (to.1 - from.1).mul_add(t, from.1),
            );
        }
    }

    /// A dotted line across at height `y` on the grid, a dot every `every`.
    fn dotted(&mut self, y: f64, every: usize) {
        for x in (0..self.width()).step_by(every.max(1)) {
            self.set(GRID, x as f64, y);
        }
    }

    fn cells(&self) -> Vec<Vec<Cell>> {
        (0..self.rows)
            .map(|row| {
                (0..self.cols)
                    .map(|col| {
                        let at = row * self.cols + col;
                        let [a, b, grid] = [
                            self.layers[A][at],
                            self.layers[B][at],
                            self.layers[GRID][at],
                        ];
                        let ink = if a != 0 {
                            Ink::A
                        } else if b != 0 {
                            Ink::B
                        } else if grid != 0 {
                            Ink::Grid
                        } else {
                            Ink::Blank
                        };
                        Cell {
                            glyph: braille(a | b | grid),
                            ink,
                        }
                    })
                    .collect()
            })
            .collect()
    }
}

const A: usize = 0;
const B: usize = 1;
const GRID: usize = 2;

/// A zoom that draws: the scope's own, or 1 when it is not a number.
fn zoom_of(reading: &ScopeReading) -> f64 {
    if reading.zoom.is_finite() && reading.zoom > 0.0 {
        reading.zoom
    } else {
        1.0
    }
}

/// The screen of `reading`, `cols` × `rows` cells.
#[must_use]
pub fn scope(reading: &ScopeReading, cols: usize, rows: usize) -> Vec<Vec<Cell>> {
    let mut dots = Dots::new(cols, rows);
    if cols == 0 || rows == 0 {
        return dots.cells();
    }
    let zoom = zoom_of(reading);
    if reading.mode == "xy" {
        figure(&mut dots, reading, zoom);
    } else {
        sweep(&mut dots, reading, zoom);
    }
    dots.cells()
}

/// Inputs a and b across the screen.
fn sweep(dots: &mut Dots, reading: &ScopeReading, zoom: f64) {
    let half = (dots.height() as f64 - 1.0) / 2.0;
    let to_y = |value: f64| (value * zoom).mul_add(-half, half).clamp(0.0, half * 2.0);
    dots.dotted(to_y(0.0), 2);
    if reading.mode == "trigger" {
        dots.dotted(to_y(reading.trigger), 6);
    }
    let capacity = (reading.points as usize).max(1);
    let across = (dots.width() as f64 - 1.0).max(0.0);
    let to_x = |point: usize| {
        if capacity <= 1 {
            0.0
        } else {
            point as f64 * across / (capacity - 1) as f64
        }
    };
    for (layer, points) in [(B, &reading.b), (A, &reading.a)] {
        // A rolling screen fills from the right; a sweep from the left.
        let offset = if reading.mode == "free" {
            capacity.saturating_sub(points.len())
        } else {
            0
        };
        let mut before: Option<(f64, f64)> = None;
        for (index, [low, high]) in points.iter().enumerate().take(capacity) {
            let x = to_x(index + offset);
            let (top, bottom) = (to_y(*high), to_y(*low));
            dots.line(layer, (x, top), (x, bottom));
            let middle = f64::midpoint(top, bottom);
            if let Some(from) = before {
                dots.line(layer, from, (x, middle));
            }
            before = Some((x, middle));
        }
    }
}

/// Input a across against b up, on square axes.
fn figure(dots: &mut Dots, reading: &ScopeReading, zoom: f64) {
    let (centre_x, centre_y) = (
        (dots.width() as f64 - 1.0) / 2.0,
        (dots.height() as f64 - 1.0) / 2.0,
    );
    let reach = centre_x.min(centre_y);
    dots.dotted(centre_y, 2);
    for y in (0..dots.height()).step_by(2) {
        dots.set(GRID, centre_x, y as f64);
    }
    let place = |[a, b]: [f64; 2]| {
        (
            (a * zoom).clamp(-1.0, 1.0).mul_add(reach, centre_x),
            (-b * zoom).clamp(-1.0, 1.0).mul_add(reach, centre_y),
        )
    };
    let mut before: Option<(f64, f64)> = None;
    for pair in &reading.xy {
        let at = place(*pair);
        match before {
            Some(from) => dots.line(A, from, at),
            None => dots.set(A, at.0, at.1),
        }
        before = Some(at);
    }
}

/// The line under the screen with input a's frequency, when it repeats:
/// see [`label`].
#[must_use]
pub fn full_label(reading: &ScopeReading) -> String {
    let label = label(reading);
    match reading.input_a.hz {
        Some(hz) => format!("{label} · {}", hertz(hz as f32)),
        None => label,
    }
}

/// The line under the screen: how the sweep was drawn (`trig'd` on a
/// trigger, `auto` without one, `sweep…` while one is under way, `roll`,
/// `xy`), the span and the zoom.
#[must_use]
pub fn label(reading: &ScopeReading) -> String {
    let state = match reading.mode.as_str() {
        "xy" if reading.sweeping.is_some() => "xy…",
        "xy" => "xy",
        "free" => "roll",
        _ if reading.sweeping.is_some() => "sweep…",
        _ if reading.triggered => "trig'd",
        _ => "auto",
    };
    format!(
        "{state} {} ×{}",
        seconds(reading.span as f32),
        number(zoom_of(reading) as f32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_wall::protocol::{GateMeasured, Measured, Rhythm};

    fn quiet() -> Measured {
        Measured {
            connected: true,
            waiting: false,
            last: 0.0,
            low: 0.0,
            high: 0.0,
            mean: 0.0,
            rms_db: -120.0,
            peak_db: -120.0,
            hz: None,
            rhythm: Rhythm::None,
            shape: None,
            gate: GateMeasured::default(),
            min: None,
            max: None,
            faults: 0,
            faults_since_reset: 0,
        }
    }

    fn reading(mode: &str) -> ScopeReading {
        ScopeReading {
            mode: mode.to_string(),
            span: 0.02,
            zoom: 1.0,
            trigger: 0.0,
            triggered: true,
            points: 96,
            sweeping: None,
            a: Vec::new(),
            b: Vec::new(),
            xy: Vec::new(),
            input_a: quiet(),
            input_b: quiet(),
            words: String::new(),
        }
    }

    /// The rows each column's a-dots reach, top to bottom, in dots.
    fn ink_rows(cells: &[Vec<Cell>], ink: Ink) -> Vec<usize> {
        let mut rows = Vec::new();
        for (row, line) in cells.iter().enumerate() {
            if line.iter().any(|cell| cell.ink == ink) {
                rows.push(row);
            }
        }
        rows
    }

    #[test]
    fn a_flat_trace_lies_on_its_level_and_off_screen_values_on_the_edge() {
        let mut flat = reading("trigger");
        flat.a = vec![[0.5, 0.5]; 96];
        let cells = scope(&flat, 24, 10);
        // +0.5 at ×1 is a quarter of the way down a 40-dot screen.
        let rows = ink_rows(&cells, Ink::A);
        assert_eq!(rows, vec![2], "{rows:?}");
        assert!(cells[2].iter().all(|cell| cell.ink == Ink::A));
        // At ×4 it is off the top: drawn on the edge.
        flat.zoom = 4.0;
        let cells = scope(&flat, 24, 10);
        assert_eq!(ink_rows(&cells, Ink::A), vec![0]);
        // The zero line shows where nothing is drawn.
        assert!(
            cells[4]
                .iter()
                .chain(&cells[5])
                .any(|cell| cell.ink == Ink::Grid)
        );
    }

    #[test]
    fn a_band_fills_from_low_to_high_and_b_sits_under_a() {
        let mut band = reading("trigger");
        band.a = vec![[-1.0, 1.0]; 96];
        band.b = vec![[0.0, 0.0]; 96];
        let cells = scope(&band, 12, 6);
        for line in &cells {
            assert!(line.iter().all(|cell| cell.ink == Ink::A), "a covers b");
            assert!(line.iter().all(|cell| cell.glyph != braille(0)));
        }
    }

    #[test]
    fn a_rolling_screen_fills_from_the_right() {
        let mut rolling = reading("free");
        rolling.a = vec![[0.0, 0.0]; 10];
        let cells = scope(&rolling, 24, 6);
        let middle = &cells[2];
        assert!(middle[23].ink == Ink::A || cells[3][23].ink == Ink::A);
        assert!(
            cells.iter().all(|line| line[0].ink != Ink::A),
            "left is empty"
        );
    }

    #[test]
    fn a_sweep_under_way_is_drawn_from_the_left() {
        let mut partial = reading("trigger");
        partial.a = vec![[0.0, 0.0]; 10];
        let cells = scope(&partial, 24, 6);
        assert!(cells[3][0].ink == Ink::A, "it starts at the left");
        assert!(
            cells.iter().all(|line| line[23].ink != Ink::A),
            "and stops short"
        );
    }

    #[test]
    fn xy_draws_a_round_figure() {
        let mut figure = reading("xy");
        figure.xy = (0..96)
            .map(|step| {
                let angle = std::f64::consts::TAU * f64::from(step) / 96.0;
                [0.9 * angle.cos(), 0.9 * angle.sin()]
            })
            .collect();
        let cells = scope(&figure, 20, 10);
        // A ring: the middle is empty, the edges of the square are not.
        assert!(cells[4][9].ink != Ink::A && cells[5][10].ink != Ink::A);
        let inked = cells
            .iter()
            .flatten()
            .filter(|cell| cell.ink == Ink::A)
            .count();
        assert!(inked > 20, "{inked}");
        // On square axes: as many rows as columns' worth of dots.
        let rows = ink_rows(&cells, Ink::A);
        assert!(rows.len() >= 9, "{rows:?}");
    }

    #[test]
    fn every_size_draws_and_nothing_is_left_blank_for_a_cable() {
        let mut busy = reading("trigger");
        busy.a = (0..96)
            .map(|i| [-0.3 - f64::from(i) / 200.0, 0.4])
            .collect();
        busy.b = vec![[f64::NAN, f64::INFINITY]; 96];
        for (cols, rows) in [(0, 0), (1, 1), (3, 2), (24, 13), (60, 30)] {
            let cells = scope(&busy, cols, rows);
            assert_eq!(cells.len(), rows);
            assert!(cells.iter().all(|line| line.len() == cols));
            assert!(cells.iter().flatten().all(|cell| cell.glyph != ' '));
        }
    }

    #[test]
    fn the_label_says_how_the_sweep_was_drawn() {
        let mut shown = reading("trigger");
        assert_eq!(label(&shown), "trig'd 20 ms ×1");
        shown.triggered = false;
        shown.zoom = 4.0;
        assert_eq!(label(&shown), "auto 20 ms ×4");
        shown.mode = "free".to_string();
        shown.span = 2.0;
        assert_eq!(label(&shown), "roll 2 s ×4");
        shown.mode = "xy".to_string();
        shown.zoom = f64::NAN;
        assert_eq!(label(&shown), "xy 2 s ×1");
    }

    #[test]
    fn the_longest_label_fits_under_the_screen() {
        let mut shown = reading("trigger");
        shown.span = 0.0025;
        shown.zoom = 0.125;
        shown.input_a.hz = Some(12_345.6);
        let label = full_label(&shown);
        assert!(
            label.starts_with("trig'd 2.5 ms ×0.1") && label.ends_with(" · 12.35 kHz"),
            "{label}"
        );
        let room = usize::from(super::super::geometry::SCOPE_W - 2);
        assert!(label.chars().count() <= room, "{label}");
    }
}
