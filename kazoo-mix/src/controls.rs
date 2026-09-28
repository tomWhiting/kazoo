//! Channel-strip and master-section control model.
//!
//! These are plain values with no audio or UI dependencies. The desk edits
//! them, the shared state carries them to the audio callback, and the engine
//! turns them into gains. Every setter clamps, and non-finite input falls back
//! to the control's default, so no control value can ever reach the audio
//! path out of range.

use kazoo_core::Pan;

use crate::eq::{EQ_RANGE_DB, EqSettings};

/// Fader value treated as fully off (−∞ dB).
pub const FADER_OFF_DB: f32 = -90.0;

/// Maximum fader gain in decibels.
pub const FADER_MAX_DB: f32 = 10.0;

/// Trim range in decibels (symmetric).
pub const TRIM_RANGE_DB: f32 = 20.0;

/// Default aux-return level (linear).
pub const DEFAULT_AUX_RETURN: f32 = 0.7;

/// Console-style fader law as `(position, dB)` breakpoints.
///
/// Position 0 is the bottom of the throw, 1 the top. Unity (0 dB) sits at
/// three quarters of the throw, with fine resolution around it and a long
/// taper toward −∞, like a real long-throw fader.
const FADER_LAW: [(f32, f32); 8] = [
    (0.0, FADER_OFF_DB),
    (0.05, -60.0),
    (0.15, -40.0),
    (0.25, -30.0),
    (0.38, -20.0),
    (0.55, -10.0),
    (0.75, 0.0),
    (1.0, FADER_MAX_DB),
];

/// Convert a fader position (0‥1) to decibels.
#[must_use]
pub fn fader_position_to_db(position: f32) -> f32 {
    let position = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    for pair in FADER_LAW.windows(2) {
        let (p0, d0) = pair[0];
        let (p1, d1) = pair[1];
        if position <= p1 {
            let t = (position - p0) / (p1 - p0);
            return (d1 - d0).mul_add(t, d0);
        }
    }
    FADER_MAX_DB
}

/// Convert decibels to a fader position (0‥1).
#[must_use]
pub fn fader_db_to_position(db: f32) -> f32 {
    let db = if db.is_finite() {
        db.clamp(FADER_OFF_DB, FADER_MAX_DB)
    } else {
        FADER_OFF_DB
    };
    for pair in FADER_LAW.windows(2) {
        let (p0, d0) = pair[0];
        let (p1, d1) = pair[1];
        if db <= d1 {
            let t = (db - d0) / (d1 - d0);
            return (p1 - p0).mul_add(t, p0);
        }
    }
    1.0
}

/// Convert a fader dB value to linear gain; the off position is exactly zero.
#[must_use]
pub fn fader_db_to_gain(db: f32) -> f32 {
    if !db.is_finite() || db <= FADER_OFF_DB {
        0.0
    } else {
        10.0_f32.powf(db.min(FADER_MAX_DB) / 20.0)
    }
}

/// Convert a decibel value to linear gain (no off position).
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    if db.is_finite() {
        10.0_f32.powf(db / 20.0)
    } else {
        0.0
    }
}

/// Every control on a channel strip, in top-to-bottom desk order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StripControl {
    /// Input trim.
    Trim,
    /// High-shelf EQ.
    EqHigh,
    /// Mid peaking EQ.
    EqMid,
    /// Low-shelf EQ.
    EqLow,
    /// Aux (reverb) send.
    Aux,
    /// Pan.
    Pan,
    /// Channel fader.
    Fader,
}

impl StripControl {
    /// All strip controls in desk order.
    pub const ALL: [Self; 7] = [
        Self::Trim,
        Self::EqHigh,
        Self::EqMid,
        Self::EqLow,
        Self::Aux,
        Self::Pan,
        Self::Fader,
    ];

    /// Short engraved label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trim => "TRM",
            Self::EqHigh => "HI",
            Self::EqMid => "MID",
            Self::EqLow => "LO",
            Self::Aux => "AUX",
            Self::Pan => "PAN",
            Self::Fader => "FDR",
        }
    }

    /// Next control down the strip, wrapping to the top.
    #[must_use]
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Previous control up the strip, wrapping to the bottom.
    #[must_use]
    pub fn previous(self) -> Self {
        let idx = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Every control on the master section, in desk order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MasterControl {
    /// Aux (reverb) return level.
    AuxReturn,
    /// Master fader.
    Fader,
}

impl MasterControl {
    /// All master controls in desk order.
    pub const ALL: [Self; 2] = [Self::AuxReturn, Self::Fader];

    /// Short engraved label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AuxReturn => "RTN",
            Self::Fader => "MST",
        }
    }

    /// The other master control (there are only two).
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::AuxReturn => Self::Fader,
            Self::Fader => Self::AuxReturn,
        }
    }
}

/// How far one adjustment moves a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A small, precise nudge.
    Fine,
    /// A large jump.
    Coarse,
}

/// Channel-strip controls applied during mixing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelControls {
    /// Input trim in dB (±[`TRIM_RANGE_DB`]).
    pub trim_db: f32,
    /// Three-band EQ.
    pub eq: EqSettings,
    /// Post-fader aux send level (linear, 0‥1).
    pub aux_send: f32,
    /// Stereo pan position.
    pub pan: Pan,
    /// Fader in dB ([`FADER_OFF_DB`]‥[`FADER_MAX_DB`]).
    pub fader_db: f32,
    /// Hard mute.
    pub muted: bool,
    /// Solo state.
    pub soloed: bool,
}

impl Default for ChannelControls {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ChannelControls {
    /// Unity, flat, centred, no send.
    pub const DEFAULT: Self = Self {
        trim_db: 0.0,
        eq: EqSettings::FLAT,
        aux_send: 0.0,
        pan: Pan::CENTER,
        fader_db: 0.0,
        muted: false,
        soloed: false,
    };

    /// Return a copy with every field in range; non-finite values become the
    /// field's default.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            trim_db: finite_clamp(self.trim_db, -TRIM_RANGE_DB, TRIM_RANGE_DB, 0.0),
            eq: self.eq.clamped(),
            aux_send: finite_clamp(self.aux_send, 0.0, 1.0, 0.0),
            pan: Pan::new(self.pan.value()),
            fader_db: finite_clamp(self.fader_db, FADER_OFF_DB, FADER_MAX_DB, 0.0),
            muted: self.muted,
            soloed: self.soloed,
        }
    }

    /// Nudge one control by `steps` increments (negative moves down/left).
    pub fn adjust(&mut self, control: StripControl, steps: i32, step: Step) {
        let n = steps as f32;
        let coarse = step == Step::Coarse;
        match control {
            StripControl::Trim => {
                let inc = if coarse { 3.0 } else { 0.5 };
                self.trim_db = n.mul_add(inc, self.trim_db);
            }
            StripControl::EqHigh => {
                self.eq.high_db = n.mul_add(eq_step(coarse), self.eq.high_db);
            }
            StripControl::EqMid => self.eq.mid_db = n.mul_add(eq_step(coarse), self.eq.mid_db),
            StripControl::EqLow => self.eq.low_db = n.mul_add(eq_step(coarse), self.eq.low_db),
            StripControl::Aux => {
                let inc = if coarse { 0.1 } else { 0.02 };
                self.aux_send = n.mul_add(inc, self.aux_send);
            }
            StripControl::Pan => {
                let inc = if coarse { 0.25 } else { 0.05 };
                self.pan = Pan::new(n.mul_add(inc, self.pan.value()));
            }
            StripControl::Fader => self.fader_db = nudge_fader(self.fader_db, n, coarse),
        }
        *self = self.sanitized();
    }

    /// Return one control to its default.
    pub const fn reset(&mut self, control: StripControl) {
        let d = Self::DEFAULT;
        match control {
            StripControl::Trim => self.trim_db = d.trim_db,
            StripControl::EqHigh => self.eq.high_db = d.eq.high_db,
            StripControl::EqMid => self.eq.mid_db = d.eq.mid_db,
            StripControl::EqLow => self.eq.low_db = d.eq.low_db,
            StripControl::Aux => self.aux_send = d.aux_send,
            StripControl::Pan => self.pan = d.pan,
            StripControl::Fader => self.fader_db = d.fader_db,
        }
    }

    /// Current value of a control normalised to 0‥1 (for drawing knobs).
    #[must_use]
    pub fn normalized(&self, control: StripControl) -> f32 {
        match control {
            StripControl::Trim => (self.trim_db + TRIM_RANGE_DB) / (2.0 * TRIM_RANGE_DB),
            StripControl::EqHigh => eq_normalized(self.eq.high_db),
            StripControl::EqMid => eq_normalized(self.eq.mid_db),
            StripControl::EqLow => eq_normalized(self.eq.low_db),
            StripControl::Aux => self.aux_send,
            StripControl::Pan => (self.pan.value() + 1.0) * 0.5,
            StripControl::Fader => fader_db_to_position(self.fader_db),
        }
    }

    /// Set the fader from a throw position (0‥1), e.g. a mouse drag.
    pub fn set_fader_position(&mut self, position: f32) {
        self.fader_db = fader_position_to_db(position);
        *self = self.sanitized();
    }
}

/// Master-section controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasterControls {
    /// Master fader in dB.
    pub fader_db: f32,
    /// Aux (reverb) return level, linear 0‥1.
    pub aux_return: f32,
}

impl Default for MasterControls {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl MasterControls {
    /// Unity master, default return level.
    pub const DEFAULT: Self = Self {
        fader_db: 0.0,
        aux_return: DEFAULT_AUX_RETURN,
    };

    /// Return a copy with every field in range.
    #[must_use]
    pub const fn sanitized(self) -> Self {
        Self {
            fader_db: finite_clamp(self.fader_db, FADER_OFF_DB, FADER_MAX_DB, 0.0),
            aux_return: finite_clamp(self.aux_return, 0.0, 1.0, DEFAULT_AUX_RETURN),
        }
    }

    /// Nudge one master control.
    pub fn adjust(&mut self, control: MasterControl, steps: i32, step: Step) {
        let n = steps as f32;
        let coarse = step == Step::Coarse;
        match control {
            MasterControl::AuxReturn => {
                let inc = if coarse { 0.1 } else { 0.02 };
                self.aux_return = n.mul_add(inc, self.aux_return);
            }
            MasterControl::Fader => self.fader_db = nudge_fader(self.fader_db, n, coarse),
        }
        *self = self.sanitized();
    }

    /// Return one master control to its default.
    pub const fn reset(&mut self, control: MasterControl) {
        match control {
            MasterControl::AuxReturn => self.aux_return = Self::DEFAULT.aux_return,
            MasterControl::Fader => self.fader_db = Self::DEFAULT.fader_db,
        }
    }

    /// Set the master fader from a throw position (0‥1).
    pub fn set_fader_position(&mut self, position: f32) {
        self.fader_db = fader_position_to_db(position);
        *self = self.sanitized();
    }

    /// Current value of a control normalised to 0‥1.
    #[must_use]
    pub fn normalized(&self, control: MasterControl) -> f32 {
        match control {
            MasterControl::AuxReturn => self.aux_return,
            MasterControl::Fader => fader_db_to_position(self.fader_db),
        }
    }
}

const fn eq_step(coarse: bool) -> f32 {
    if coarse { 3.0 } else { 0.5 }
}

fn eq_normalized(db: f32) -> f32 {
    (db + EQ_RANGE_DB) / (2.0 * EQ_RANGE_DB)
}

/// Move a fader by position steps so the feel matches the drawn throw.
fn nudge_fader(fader_db: f32, steps: f32, coarse: bool) -> f32 {
    let inc = if coarse { 0.05 } else { 0.01 };
    let position = steps.mul_add(inc, fader_db_to_position(fader_db));
    let db = fader_position_to_db(position);
    // Snap to unity when a single nudge crosses it, so 0 dB is always
    // reachable exactly from the keyboard. Multi-step jumps pass straight
    // through.
    let crossed_unity = (fader_db < 0.0 && db > 0.0) || (fader_db > 0.0 && db < 0.0);
    if crossed_unity && steps.abs() <= 1.0 {
        0.0
    } else {
        db
    }
}

const fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::assert_float_eq;

    #[test]
    fn fader_law_round_trips_at_every_breakpoint() {
        for (position, db) in FADER_LAW {
            assert!((fader_position_to_db(position) - db).abs() < 1e-4);
            assert!((fader_db_to_position(db) - position).abs() < 1e-4);
        }
    }

    #[test]
    fn fader_law_is_monotonic() {
        let mut last = f32::NEG_INFINITY;
        for step in 0..=1_000 {
            let db = fader_position_to_db(step as f32 / 1_000.0);
            assert!(db >= last);
            last = db;
        }
    }

    #[test]
    fn fader_off_is_exact_silence_and_unity_is_one() {
        assert_float_eq(fader_db_to_gain(FADER_OFF_DB), 0.0);
        assert_float_eq(fader_db_to_gain(f32::NAN), 0.0);
        assert!((fader_db_to_gain(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fader_positions_outside_range_clamp() {
        assert_float_eq(fader_position_to_db(-3.0), FADER_OFF_DB);
        assert_float_eq(fader_position_to_db(7.0), FADER_MAX_DB);
        assert_float_eq(fader_position_to_db(f32::NAN), FADER_OFF_DB);
    }

    #[test]
    fn fader_nudge_snaps_to_unity_when_crossing() {
        let mut controls = ChannelControls {
            fader_db: -0.4,
            ..ChannelControls::DEFAULT
        };
        controls.adjust(StripControl::Fader, 1, Step::Fine);
        assert_float_eq(controls.fader_db, 0.0);
        controls.adjust(StripControl::Fader, 1, Step::Fine);
        assert!(controls.fader_db > 0.0);
    }

    #[test]
    fn fader_can_reach_off_and_max_from_keyboard() {
        let mut controls = ChannelControls::DEFAULT;
        controls.adjust(StripControl::Fader, -100, Step::Coarse);
        assert_float_eq(controls.fader_db, FADER_OFF_DB);
        controls.adjust(StripControl::Fader, 100, Step::Coarse);
        assert_float_eq(controls.fader_db, FADER_MAX_DB);
    }

    #[test]
    fn adjustments_clamp_every_control() {
        let mut controls = ChannelControls::DEFAULT;
        for control in StripControl::ALL {
            controls.adjust(control, 1_000, Step::Coarse);
        }
        assert_float_eq(controls.trim_db, TRIM_RANGE_DB);
        assert_float_eq(controls.eq.high_db, EQ_RANGE_DB);
        assert_float_eq(controls.aux_send, 1.0);
        assert_float_eq(controls.pan.value(), 1.0);
        for control in StripControl::ALL {
            controls.adjust(control, -1_000, Step::Coarse);
        }
        assert_float_eq(controls.trim_db, -TRIM_RANGE_DB);
        assert_float_eq(controls.eq.low_db, -EQ_RANGE_DB);
        assert_float_eq(controls.aux_send, 0.0);
        assert_float_eq(controls.pan.value(), -1.0);
    }

    #[test]
    fn reset_restores_defaults_per_control() {
        let mut controls = ChannelControls::DEFAULT;
        for control in StripControl::ALL {
            controls.adjust(control, 3, Step::Coarse);
        }
        for control in StripControl::ALL {
            controls.reset(control);
        }
        assert_eq!(controls, ChannelControls::DEFAULT);
    }

    #[test]
    fn sanitize_replaces_non_finite_values() {
        let controls = ChannelControls {
            trim_db: f32::NAN,
            aux_send: f32::INFINITY,
            fader_db: f32::NEG_INFINITY,
            ..ChannelControls::DEFAULT
        }
        .sanitized();
        assert_float_eq(controls.trim_db, 0.0);
        assert_float_eq(controls.aux_send, 0.0);
        assert_float_eq(controls.fader_db, 0.0);
    }

    #[test]
    fn normalized_values_stay_in_unit_range() {
        let mut controls = ChannelControls::DEFAULT;
        for steps in [-1_000, -3, 0, 3, 1_000] {
            for control in StripControl::ALL {
                controls.adjust(control, steps, Step::Coarse);
                let v = controls.normalized(control);
                assert!((0.0..=1.0).contains(&v), "{control:?} -> {v}");
            }
        }
    }

    #[test]
    fn strip_control_navigation_wraps() {
        assert_eq!(StripControl::Fader.next(), StripControl::Trim);
        assert_eq!(StripControl::Trim.previous(), StripControl::Fader);
        let mut control = StripControl::Trim;
        for _ in 0..StripControl::ALL.len() {
            control = control.next();
        }
        assert_eq!(control, StripControl::Trim);
    }

    #[test]
    fn master_controls_adjust_clamp_and_reset() {
        let mut master = MasterControls::DEFAULT;
        master.adjust(MasterControl::AuxReturn, 1_000, Step::Coarse);
        assert_float_eq(master.aux_return, 1.0);
        master.adjust(MasterControl::Fader, -1_000, Step::Coarse);
        assert_float_eq(master.fader_db, FADER_OFF_DB);
        master.reset(MasterControl::Fader);
        master.reset(MasterControl::AuxReturn);
        assert_eq!(master, MasterControls::DEFAULT);
        assert_eq!(MasterControl::Fader.toggled(), MasterControl::AuxReturn);
    }
}
