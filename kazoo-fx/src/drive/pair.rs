//! The stereo pair of oversampled circuits every circuit model is built on.
//!
//! Each channel is its own copy of the circuit behind its own
//! [`Oversampler`]; [`Pair::prepare`] picks the factor that takes the host's
//! rate to the model's target, and [`Pair::process`] runs one frame through
//! both. What the models keep to themselves is only their circuit.

use super::oversample::{Oversampler, factor_for};

/// A circuit one channel runs.
pub trait Circuit: Copy + Default + std::fmt::Debug {
    /// Forget all state: capacitors discharged, solvers at rest.
    fn reset(&mut self);
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel<C> {
    oversampler: Oversampler,
    circuit: C,
}

/// Two channels of circuit `C`, each oversampled.
#[derive(Debug, Clone, Copy)]
pub struct Pair<C> {
    channels: [Channel<C>; 2],
    anti_alias: bool,
    rate: f64,
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
        }
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
            channel.circuit.reset();
        }
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
        for (index, (channel, sample)) in self.channels.iter_mut().zip(frame).enumerate() {
            let circuit = &mut channel.circuit;
            out[index] = channel
                .oversampler
                .process(sample, |x| tick(index, circuit, x));
        }
        out
    }
}
