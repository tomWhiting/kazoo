//! The stereo pair of oversampled circuits every circuit model is built on.
//!
//! Each channel is its own copy of the circuit behind its own
//! [`Oversampler`]; [`Pair::prepare`] picks the factor that takes the host's
//! rate to the model's target, and [`Pair::process`] runs one frame through
//! both. A model with a mix knob mixes at the high rate, so its wet signal
//! alone comes down through a second decimator of its own
//! ([`Pair::process_wet`]). What the models keep to themselves is only their
//! circuit.

use super::oversample::{MAX_FACTOR, Oversampler, factor_for};

/// A circuit one channel runs.
pub trait Circuit: Copy + Default + std::fmt::Debug {
    /// Whether the circuit, from reset, turns input that is exactly zero
    /// into output that is exactly zero with its state left as reset left
    /// it. A pair whose circuit says so skips a channel that has heard
    /// nothing but zeros since reset: an unplugged side costs nothing, and
    /// the output is bit for bit what running it would give.
    const QUIET_AT_REST: bool = false;

    /// Whether the model's tick treats both channels alike, so two
    /// channels in the same state given the same input give the same
    /// output and end in the same state. A pair whose circuit says so runs
    /// one channel and copies it while both hear the same samples (a mono
    /// signal on both sides): the second channel takes the first's state
    /// when the sides first differ, which is exactly the state it would
    /// have reached running itself.
    const SIDES_ALIKE: bool = false;

    /// Forget all state: capacitors discharged, solvers at rest.
    fn reset(&mut self);
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel<C> {
    oversampler: Oversampler,
    /// The way down for the wet signal alone: the same filters and pad as
    /// `oversampler`'s, so it comes out exactly as late. Only its
    /// decimating half is used.
    wet: Oversampler,
    circuit: C,
}

/// Two channels of circuit `C`, each oversampled.
#[derive(Debug, Clone, Copy)]
pub struct Pair<C> {
    channels: [Channel<C>; 2],
    anti_alias: bool,
    rate: f64,
    /// Whether the wet decimators ran on the last frame: one that starts
    /// again starts silent, never from audio it last heard long ago.
    wet_live: bool,
    /// Per channel, whether it has heard only zeros since reset.
    idle: [bool; 2],
    /// Whether channels are skipped, and whether the right is the left.
    sharing: Sharing,
}

/// How a pair may save work across its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sharing {
    /// Both channels have heard the same samples since reset, so the
    /// right channel's state is the left's.
    Twinned,
    /// The channels have heard different samples; each runs itself
    /// (an idle one may still be skipped).
    Apart,
    /// Every channel runs, always: only for the tests that prove the
    /// skips change nothing.
    #[cfg(test)]
    Off,
}

impl<C: Circuit> Pair<C> {
    /// A pair that anti-aliases (oversamples, and lets the circuit use its
    /// antiderivatives) unless `anti_alias` is false: the naive path the
    /// aliasing tests compare against.
    #[must_use]
    pub fn new(anti_alias: bool) -> Self {
        Self {
            channels: [Channel::default(); 2],
            anti_alias,
            rate: 0.0,
            wet_live: false,
            idle: [true; 2],
            sharing: Sharing::Twinned,
        }
    }

    /// The same pair, but every channel runs itself, even while it hears
    /// nothing or the same as the other: what the skips are checked
    /// against.
    #[cfg(test)]
    #[must_use]
    pub const fn running_idle(mut self) -> Self {
        self.sharing = Sharing::Off;
        self
    }

    /// Whether channel `index` may be skipped for input `sample`: its
    /// circuit is quiet at rest and it has heard only zeros since reset.
    /// The first sample that is not zero wakes it for good, until reset.
    fn skips(&mut self, index: usize, sample: f32) -> bool {
        #[cfg(test)]
        if self.sharing == Sharing::Off {
            return false;
        }
        if !(C::QUIET_AT_REST && self.idle[index]) {
            return false;
        }
        if sample == 0.0 {
            return true;
        }
        self.idle[index] = false;
        false
    }

    /// Whether this pair anti-aliases.
    #[must_use]
    pub const fn anti_alias(&self) -> bool {
        self.anti_alias
    }

    /// Oversample from `base_rate` towards `target` (not at all on the
    /// naive path), for circuits whose antiderivatives delay by `extra`
    /// samples at their own rate (none on the naive path, which does not
    /// use them), silence everything, and return the rate the circuits now
    /// run at.
    pub fn prepare(&mut self, base_rate: f32, target: f32, extra: f64) -> f64 {
        let (factor, extra) = if self.anti_alias {
            (factor_for(base_rate, target), extra)
        } else {
            (1, 0.0)
        };
        for channel in &mut self.channels {
            channel.oversampler.configure(factor, base_rate, extra);
            channel.wet.configure(factor, base_rate, extra);
        }
        self.rate = f64::from(base_rate) * factor as f64;
        self.reset();
        self.rate
    }

    /// How many host samples the pair delays by: the oversampler's round
    /// trip and the circuits' antiderivatives, padded out to a whole
    /// number.
    #[must_use]
    pub const fn latency(&self) -> usize {
        self.channels[0].oversampler.latency()
    }

    /// The rate the circuits run at.
    #[must_use]
    pub const fn rate(&self) -> f64 {
        self.rate
    }

    /// Both circuits, left then right.
    pub fn circuits(&mut self) -> impl Iterator<Item = &mut C> {
        self.channels.iter_mut().map(|channel| &mut channel.circuit)
    }

    /// Silence both oversamplers and both circuits.
    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.oversampler.reset();
            channel.wet.reset();
            channel.circuit.reset();
        }
        self.idle = [true; 2];
        #[cfg(test)]
        if self.sharing == Sharing::Off {
            return;
        }
        self.sharing = Sharing::Twinned;
    }

    /// Whether this frame's right channel is the left's: both have heard
    /// the same samples since reset and hear the same now. When the sides
    /// first differ, the right channel takes the left's state, the one it
    /// would have reached running itself, and runs on its own until reset.
    fn twins(&mut self, frame: [f32; 2]) -> bool {
        if !(C::SIDES_ALIKE && self.sharing == Sharing::Twinned) {
            return false;
        }
        if frame[0].to_bits() == frame[1].to_bits() {
            if frame[0] != 0.0 {
                self.idle = [false; 2];
            }
            return true;
        }
        self.sharing = Sharing::Apart;
        self.channels[1] = self.channels[0];
        self.idle[1] = self.idle[0];
        false
    }

    /// One frame: `tick` is called for every oversampled sample of the left
    /// channel in time order, then of the right, with the channel's index
    /// (0 or 1) and its circuit.
    pub fn process(
        &mut self,
        frame: [f32; 2],
        mut tick: impl FnMut(usize, &mut C, f32) -> f32,
    ) -> [f32; 2] {
        let mut out = [0.0; 2];
        if self.twins(frame) {
            let channel = &mut self.channels[0];
            let circuit = &mut channel.circuit;
            let both = channel
                .oversampler
                .process(frame[0], |x| tick(0, circuit, x));
            return [both, both];
        }
        let skipped = [self.skips(0, frame[0]), self.skips(1, frame[1])];
        for (index, (channel, sample)) in self.channels.iter_mut().zip(frame).enumerate() {
            if skipped[index] {
                continue;
            }
            let circuit = &mut channel.circuit;
            out[index] = channel
                .oversampler
                .process(sample, |x| tick(index, circuit, x));
        }
        out
    }

    /// One frame through circuits that mix at the high rate: `tick` gives
    /// the mixed sample and the wet sample alone for every oversampled
    /// sample, in the order [`Self::process`] calls it. Returns the mixed
    /// frame, then the wet frame, each brought down through its own
    /// decimator, so the two are equally late. Unless `wanted`, the wet
    /// decimators are not run and the wet frame is silence.
    pub fn process_wet(
        &mut self,
        frame: [f32; 2],
        wanted: bool,
        mut tick: impl FnMut(usize, &mut C, f32) -> [f32; 2],
    ) -> [[f32; 2]; 2] {
        if wanted && !self.wet_live {
            for channel in &mut self.channels {
                channel.wet.reset();
            }
        }
        self.wet_live = wanted;
        let mut out = [[0.0; 2]; 2];
        let skipped = [self.skips(0, frame[0]), self.skips(1, frame[1])];
        for (index, (channel, sample)) in self.channels.iter_mut().zip(frame).enumerate() {
            if skipped[index] {
                continue;
            }
            let factor = channel.oversampler.factor();
            let mut mixed = channel.oversampler.expand(sample);
            let mut alone = [0.0f32; MAX_FACTOR];
            for (value, wet) in mixed[..factor].iter_mut().zip(&mut alone) {
                [*value, *wet] = tick(index, &mut channel.circuit, *value);
            }
            out[0][index] = channel.oversampler.reduce(mixed);
            if wanted {
                out[1][index] = channel.wet.reduce(alone);
            }
        }
        out
    }
}
