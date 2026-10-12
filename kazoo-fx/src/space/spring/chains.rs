use crate::dsp::flush;

/// Three cascades of identical stretched first-order allpasses, one per
/// spring. The three are run stage by stage together: each cascade is one
/// long serial chain of multiply-adds, and interleaving three independent
/// chains lets the processor overlap them.
///
/// The stretch `K` is fractional, so a cascade's timing in seconds is the
/// same at every sample rate: each stage's `z^-K` is `N` whole samples of
/// delay followed by a first-order Thiran allpass for the fraction
/// (`d` between 0.5 and 1.5 samples, coefficient `(1 - d) / (1 + d)`),
/// which is maximally flat in delay at low frequencies. This is Parker's
/// interpolated stretched allpass (2011).
#[derive(Debug, Clone, Default)]
pub(super) struct Chains {
    /// Per cascade, `N + 1` samples of each stage's inner signal.
    pub(super) history: [Vec<f32>; 3],
    /// Per cascade, each stage's last Thiran output.
    pub(super) thiran_out: [Vec<f32>; 3],
    pub(super) live: usize,
    ring: usize,
    thiran: f32,
    position: usize,
}

impl Chains {
    /// Cascades of `stages` stages stretched by `stretch` samples (held at
    /// 1.5 or more, so each stage's delay is at least one whole sample).
    pub(super) fn new(stages: usize, stretch: f32) -> Self {
        let stretch = if stretch.is_finite() {
            stretch.max(1.5)
        } else {
            1.5
        };
        let whole = (stretch - 0.5).floor();
        let fraction = stretch - whole;
        let ring = whole as usize + 1;
        Self {
            history: std::array::from_fn(|_| vec![0.0; stages * ring]),
            thiran_out: std::array::from_fn(|_| vec![0.0; stages]),
            live: 3,
            ring,
            thiran: (1.0 - fraction) / (1.0 + fraction),
            position: 0,
        }
    }

    /// One sample through every stage of each cascade, cascade `i` taking
    /// `inputs[i]` with coefficient `coeffs[i]`.
    pub(super) fn process(&mut self, inputs: [f32; 3], coeffs: [f32; 3]) -> [f32; 3] {
        // Specializing the slice keeps the interleaved chains unrolled.
        if self.live == 3 {
            self.process_live::<3>(inputs, coeffs)
        } else {
            self.process_live::<2>(inputs, coeffs)
        }
    }

    fn process_live<const LIVE: usize>(&mut self, inputs: [f32; 3], coeffs: [f32; 3]) -> [f32; 3] {
        let mut x = inputs;
        // The slot about to be written holds the inner signal N + 1 samples
        // back; the next slot holds it N samples back.
        let oldest = self.position;
        let newer = if oldest + 1 == self.ring {
            0
        } else {
            oldest + 1
        };
        let eta = self.thiran;
        let ring = self.ring;
        let [h0, h1, h2] = &mut self.history;
        let [t0, t1, t2] = &mut self.thiran_out;
        let stages = h0
            .chunks_exact_mut(ring)
            .zip(h1.chunks_exact_mut(ring))
            .zip(h2.chunks_exact_mut(ring))
            .zip(t0.iter_mut().zip(t1.iter_mut()).zip(t2.iter_mut()));
        for (((one, two), three), ((last0, last1), last2)) in stages {
            let mut cells = [(one, last0), (two, last1), (three, last2)];
            for (((cell, last), x), a) in cells[..LIVE].iter_mut().zip(&mut x).zip(coeffs) {
                // The fractional delay: Thiran's first-order allpass.
                let delayed = eta.mul_add(cell[newer] - **last, cell[oldest]);
                **last = delayed;
                // The stretched allpass around it.
                let w = (-a).mul_add(delayed, *x);
                cell[oldest] = w;
                *x = a.mul_add(w, delayed);
            }
        }
        self.position = newer;
        x[LIVE..].fill(0.0);
        x
    }

    pub(super) fn set_live(&mut self, live: usize) {
        if live == 2 && self.live == 3 {
            self.history[2].fill(0.0);
            self.thiran_out[2].fill(0.0);
        }
        self.live = live;
    }

    /// Zero any state that has decayed into the denormal range. Done once
    /// a block rather than at every stage: a flush inside the cascade would
    /// sit on its serial path and cost several times the cascade itself,
    /// and between sweeps the state can spend at most a block near the
    /// denormal range.
    pub(super) fn sweep(&mut self) {
        for state in self.history.iter_mut().chain(&mut self.thiran_out) {
            for value in state.iter_mut() {
                flush(value);
            }
        }
    }

    pub(super) fn clear(&mut self) {
        for state in self.history.iter_mut().chain(&mut self.thiran_out) {
            state.fill(0.0);
        }
        self.position = 0;
    }
}
