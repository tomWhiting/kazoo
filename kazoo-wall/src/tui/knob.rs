//! Knob travel for any kind the catalogue describes: where a value sits
//! along the knob's sweep, how far one press turns it, and a value as
//! people read it while the wall has not yet shown it back.
//!
//! The travel follows the catalogue's own law ([`Curve`]): a log knob moves
//! in even ratios, and a log range starting at zero is logarithmic from a
//! thousandth of its top, with zero at the very bottom.

use kazoo_wall::catalogue::Curve;
use kazoo_wall::format;
use kazoo_wall::protocol::{KnobInfo, KnobView};

/// How far one press turns a knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A two-hundredth of the travel (shifted `+` / `_`).
    Fine,
    /// A fortieth of the travel (`=` / `-`).
    Normal,
    /// An eighth of the travel (page keys, or with Alt).
    Coarse,
}

impl Step {
    /// The share of the travel one press moves a smooth knob.
    const fn travel(self) -> f64 {
        match self {
            Self::Fine => 1.0 / 200.0,
            Self::Normal => 1.0 / 40.0,
            Self::Coarse => 1.0 / 8.0,
        }
    }

    /// The positions one press moves a stepped knob spanning `span`.
    fn positions(self, span: f64) -> f64 {
        match self {
            Self::Fine | Self::Normal => 1.0,
            Self::Coarse => (span / 8.0).round().max(1.0),
        }
    }
}

/// A knob's range and law.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Travel {
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// How the sweep maps to the value.
    pub curve: Curve,
    /// Whole numbers only.
    pub stepped: bool,
}

impl Travel {
    /// The travel of a knob on the wall, with its law from the catalogue
    /// (linear when the catalogue has not said).
    #[must_use]
    pub fn of(view: &KnobView, info: Option<&KnobInfo>) -> Self {
        let (min, max) = if view.min <= view.max {
            (view.min, view.max)
        } else {
            (view.max, view.min)
        };
        Self {
            min,
            max,
            curve: info.map_or(Curve::Linear, |info| info.curve),
            stepped: view.stepped,
        }
    }

    /// `value` held to the range (and to whole numbers when stepped); a
    /// value that is not a number is the bottom of the range.
    #[must_use]
    pub fn clamp(&self, value: f64) -> f64 {
        if !value.is_finite() {
            return self.min;
        }
        let value = value.clamp(self.min, self.max);
        if self.stepped {
            value.round().clamp(self.min, self.max)
        } else {
            value
        }
    }

    /// Bottom of the logarithmic part of the range.
    fn log_floor(&self) -> f64 {
        if self.min > 0.0 {
            self.min
        } else {
            self.max * 1.0e-3
        }
    }

    /// Whether the log law applies: a positive top to travel towards.
    fn is_log(&self) -> bool {
        self.curve == Curve::Log && self.max > 0.0 && self.log_floor() > 0.0
    }

    /// Where `value` sits along the sweep, 0 to 1.
    #[must_use]
    pub fn position(&self, value: f64) -> f64 {
        let value = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        if self.is_log() {
            let floor = self.log_floor();
            if value <= floor {
                return 0.0;
            }
            return (value / floor).log(self.max / floor).clamp(0.0, 1.0);
        }
        (value - self.min) / span
    }

    /// The value at `position` (0 to 1) along the sweep.
    #[must_use]
    pub fn value_at(&self, position: f64) -> f64 {
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let value = if self.is_log() {
            if position <= 0.0 {
                self.min
            } else {
                let floor = self.log_floor();
                floor * (self.max / floor).powf(position)
            }
        } else {
            (self.max - self.min).mul_add(position, self.min)
        };
        self.clamp(value)
    }

    /// `from` turned one press of `step`, up when `up`.
    #[must_use]
    pub fn turned(&self, from: f64, up: bool, step: Step) -> f64 {
        let sign = if up { 1.0 } else { -1.0 };
        if self.stepped {
            let positions = step.positions(self.max - self.min);
            return self.clamp(self.clamp(from) + sign * positions);
        }
        self.value_at(sign.mul_add(step.travel(), self.position(from)))
    }

    /// Whether `a` and `b` are the same setting, allowing for the rounding
    /// a value picks up on the wire.
    #[must_use]
    pub fn same(&self, a: f64, b: f64) -> bool {
        let span = (self.max - self.min).abs().max(f64::EPSILON);
        (a - b).abs() <= span * 1.0e-6
    }
}

/// `value` with its unit, as people read it: the named position of a
/// stepped knob with labels, Hz or kHz, ms or s, a signed interval, or a
/// plain number with the unit after it.
#[must_use]
pub fn show(value: f64, unit: &str, labels: &[String], min: f64) -> String {
    if !labels.is_empty() {
        let index = (value - min).round();
        if index >= 0.0 {
            if let Some(label) = labels.get(index as usize) {
                return label.clone();
            }
        }
    }
    let narrow = value as f32;
    match unit {
        "" => format::number(narrow),
        "Hz" => format::hertz(narrow),
        "s" => format::seconds(narrow),
        "st" | "oct" => {
            let text = format::number(narrow);
            if value > 0.0 {
                format!("+{text} {unit}")
            } else {
                format!("{text} {unit}")
            }
        }
        other => format!("{} {other}", format::number(narrow)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn travel(min: f64, max: f64, curve: Curve, stepped: bool) -> Travel {
        Travel {
            min,
            max,
            curve,
            stepped,
        }
    }

    #[test]
    fn log_knobs_travel_in_ratios_and_come_back() {
        let cutoff = travel(20.0, 18_000.0, Curve::Log, false);
        assert!((cutoff.position(20.0)).abs() < 1e-9);
        assert!((cutoff.position(18_000.0) - 1.0).abs() < 1e-9);
        let middle = cutoff.value_at(0.5);
        assert!((middle - (20.0_f64 * 18_000.0).sqrt()).abs() < 1e-6);
        assert!((cutoff.position(middle) - 0.5).abs() < 1e-9);
        // A zero-based log range still starts at zero.
        let release = travel(0.0, 5.0, Curve::Log, false);
        assert!(release.value_at(0.0).abs() < f64::EPSILON);
        assert!(release.turned(0.0, true, Step::Normal) > 0.0);
    }

    #[test]
    fn a_press_moves_a_share_of_the_travel_and_stops_at_the_ends() {
        let level = travel(0.0, 1.0, Curve::Linear, false);
        assert!((level.turned(0.5, true, Step::Normal) - 0.525).abs() < 1e-9);
        assert!((level.turned(0.5, false, Step::Fine) - 0.495).abs() < 1e-9);
        assert!((level.turned(0.5, true, Step::Coarse) - 0.625).abs() < 1e-9);
        assert!((level.turned(0.99, true, Step::Coarse) - 1.0).abs() < f64::EPSILON);
        assert!(level.turned(0.0, false, Step::Normal).abs() < f64::EPSILON);
        assert!(level.turned(f64::NAN, true, Step::Normal) > 0.0);
    }

    #[test]
    fn stepped_knobs_move_whole_positions() {
        let steps = travel(1.0, 16.0, Curve::Linear, true);
        assert!((steps.turned(4.0, true, Step::Fine) - 5.0).abs() < f64::EPSILON);
        assert!((steps.turned(4.0, false, Step::Normal) - 3.0).abs() < f64::EPSILON);
        assert!((steps.turned(4.0, true, Step::Coarse) - 6.0).abs() < f64::EPSILON);
        assert!((steps.turned(16.0, true, Step::Coarse) - 16.0).abs() < f64::EPSILON);
        assert!((steps.clamp(3.4) - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn values_read_with_their_units() {
        assert_eq!(show(800.0, "Hz", &[], 20.0), "800 Hz");
        assert_eq!(show(1_200.0, "Hz", &[], 20.0), "1.2 kHz");
        assert_eq!(show(0.35, "s", &[], 0.0), "350 ms");
        assert_eq!(show(3.0, "st", &[], -12.0), "+3 st");
        assert_eq!(show(-2.0, "oct", &[], -4.0), "-2 oct");
        assert_eq!(show(0.4, "", &[], 0.0), "0.4");
        assert_eq!(show(4.0, "steps", &[], 1.0), "4 steps");
        let scales = vec!["chromatic".to_string(), "major".to_string()];
        assert_eq!(show(1.0, "scale", &scales, 0.0), "major");
        assert_eq!(show(9.0, "scale", &scales, 0.0), "9 scale");
    }
}
