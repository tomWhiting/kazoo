use super::{Biquad, HIGH_LEVEL, Line, Setting, Wander, cascade};
use crate::dsp::{Phasor, flush};

/// One spring: its low and high chirp loops and its wander.
#[derive(Debug, Clone, Default)]
pub(super) struct Coil {
    #[cfg(test)]
    pub(super) reads: usize,
    pub(super) line: Line,
    pub(super) high_line: Line,
    pub(super) loop_filter: [Biquad; 2],
    pub(super) out_filter: [Biquad; 2],
    pub(super) high_filter: Biquad,
    pub(super) high_top: Biquad,
    pub(super) sweep: Phasor,
    pub(super) wander: Wander,
    /// How far the corner lowpass runs ahead of the analogue one it copies,
    /// in samples (see [`super::filter_lag`]): the loop and the pickup are read
    /// that much later.
    pub(super) lag: f32,
}

impl Coil {
    pub(super) fn clear(&mut self) {
        self.line.clear();
        self.high_line.clear();
        for section in self.loop_filter.iter_mut().chain(&mut self.out_filter) {
            section.reset();
        }
        self.high_filter.reset();
        self.high_top.reset();
    }

    /// The first half of a sample: what goes into the low and the high
    /// cascade, from `input` and what has come back round the loops. `trip`
    /// is one trip down the spring in samples, `wobble` the extra delay now.
    pub(super) fn feed(
        &mut self,
        input: f32,
        trip: f32,
        wobble: f32,
        setting: Setting,
    ) -> (f32, f32) {
        #[cfg(test)]
        {
            self.reads += 2;
        }
        let returned = self.line.read(2.0f32.mul_add(trip, wobble) + self.lag);
        let shaped = cascade(&mut self.loop_filter, f64::from(returned));
        let feedback = setting.gain * shaped as f32;
        let mut low = input + feedback;
        flush(&mut low);
        let mut high = setting
            .high_gain
            .mul_add(self.high_line.read(2.0 * trip), input);
        flush(&mut high);
        (low, high)
    }

    /// The second half: the cascades' outputs go into the loops, and the
    /// pickup hears them one trip later.
    pub(super) fn pick_up(
        &mut self,
        chirped: f32,
        splashed: f32,
        trip: f32,
        wobble: f32,
        splash_on: f32,
    ) -> f32 {
        #[cfg(test)]
        {
            self.reads += 2;
        }
        self.line.push(chirped);
        let late = self.line.read(0.5f32.mul_add(wobble, trip) + self.lag);
        let drip = cascade(&mut self.out_filter, f64::from(late)) as f32;
        self.high_line.push(splashed);
        let above = self
            .high_filter
            .process(f64::from(self.high_line.read(trip)));
        let splash = self.high_top.process(above) as f32;
        (HIGH_LEVEL * splash_on).mul_add(splash, drip)
    }
}

impl super::Spring {
    pub(super) fn set_coils_live(&mut self, live: usize) {
        if live == 2 && self.lows.live == 3 {
            self.coils[2].clear();
        }
        self.lows.set_live(live);
        self.highs.set_live(live);
    }
}
