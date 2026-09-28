//! Two-times oversampling with half-band filters, for the nonlinear stages
//! of the space family (the spring's valve driver).
//!
//! A half-band lowpass cuts at a quarter of its sample rate, and every
//! second tap of its impulse response is zero, so it costs half what an
//! ordinary filter of its length would. Going up, each input sample is
//! followed by a zero and the pair is filtered; coming down, the filter
//! runs first and every second sample is kept. The taps are a sinc shaped
//! by a Kaiser window, the textbook windowed design (Kaiser, 1974), which
//! sets the stopband depth with one number, beta.
//!
//! Nothing here allocates except [`Halfband::new`] and the constructors,
//! which belong in `prepare`.

/// A half-band FIR's nonzero taps away from its centre (the centre tap is
/// always one half).
#[derive(Debug, Clone, Default)]
pub struct Halfband {
    /// `side[i]` is the tap `2i + 1` samples from the centre, either side.
    side: Vec<f32>,
}

/// The zeroth-order modified Bessel function of the first kind, by its
/// power series (converges fast for the betas a window uses).
fn bessel_i0(x: f64) -> f64 {
    let quarter = 0.25 * x * x;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        let k = f64::from(k);
        term *= quarter / (k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

impl Halfband {
    /// A half-band filter with `pairs` nonzero taps each side of the
    /// centre and a Kaiser window of `beta`. Allocates.
    #[must_use]
    pub fn new(pairs: usize, beta: f64) -> Self {
        let pairs = pairs.max(1);
        // Taps run from -(2 pairs - 1) to 2 pairs - 1 about the centre.
        let reach = (2 * pairs - 1) as f64 + 1.0;
        let window = |offset: f64| {
            let ratio = offset / reach;
            bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / bessel_i0(beta)
        };
        let side = (0..pairs)
            .map(|i| {
                let offset = (2 * i + 1) as f64;
                let angle = std::f64::consts::FRAC_PI_2 * offset;
                // 0.5 sinc(offset / 2) = sin(pi offset / 2) / (pi offset).
                let sinc = angle.sin() / (std::f64::consts::PI * offset);
                (sinc * window(offset)) as f32
            })
            .collect();
        Self { side }
    }

    fn pairs(&self) -> usize {
        self.side.len()
    }
}

/// Doubles the sample rate.
#[derive(Debug, Clone, Default)]
pub struct Up2 {
    filter: Halfband,
    /// The last `2 pairs` input samples, newest first.
    history: Vec<f32>,
}

impl Up2 {
    /// An upsampler through `filter`. Allocates.
    #[must_use]
    pub fn new(filter: Halfband) -> Self {
        let size = 2 * filter.pairs();
        Self {
            filter,
            history: vec![0.0; size],
        }
    }

    /// One input sample in, two output samples out.
    pub fn process(&mut self, input: f32) -> [f32; 2] {
        let pairs = self.filter.pairs();
        if self.history.is_empty() {
            return [input, input];
        }
        self.history.rotate_right(1);
        self.history[0] = input;
        // The zero-stuffed sequence filtered at gain two: one phase is the
        // input delayed to the centre, the other the odd taps' sum.
        let centre = self.history[pairs - 1];
        let mut between = 0.0f32;
        for (i, &tap) in self.filter.side.iter().enumerate() {
            let pair = self.history[pairs - 1 - i] + self.history[pairs + i];
            between = (2.0 * tap).mul_add(pair, between);
        }
        [between, centre]
    }

    /// Forget the past.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
    }
}

/// Halves the sample rate.
#[derive(Debug, Clone, Default)]
pub struct Down2 {
    filter: Halfband,
    /// The last `4 pairs` input samples, newest first.
    history: Vec<f32>,
}

impl Down2 {
    /// A downsampler through `filter`. Allocates.
    #[must_use]
    pub fn new(filter: Halfband) -> Self {
        let size = 4 * filter.pairs();
        Self {
            filter,
            history: vec![0.0; size],
        }
    }

    /// Two input samples in (older first), one output sample out.
    pub fn process(&mut self, input: [f32; 2]) -> f32 {
        let pairs = self.filter.pairs();
        if self.history.is_empty() {
            return input[1];
        }
        self.history.rotate_right(2);
        self.history[1] = input[0];
        self.history[0] = input[1];
        // Centre tap at 2 pairs - 1 back; odd taps either side of it.
        let centre = 2 * pairs - 1;
        let mut out = 0.5 * self.history[centre];
        for (i, &tap) in self.filter.side.iter().enumerate() {
            let offset = 2 * i + 1;
            let pair = self.history[centre - offset] + self.history[centre + offset];
            out = tap.mul_add(pair, out);
        }
        out
    }

    /// Forget the past.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAIRS: usize = 16;
    const BETA: f64 = 8.0;

    fn tone(hz: f32, rate: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|n| (std::f32::consts::TAU * hz * n as f32 / rate).sin())
            .collect()
    }

    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|x| x * x).sum::<f32>() / signal.len() as f32).sqrt()
    }

    #[test]
    fn the_taps_sum_to_a_unity_gain_filter() {
        let filter = Halfband::new(PAIRS, BETA);
        let dc = 2.0f32.mul_add(filter.side.iter().sum::<f32>(), 0.5);
        assert!((dc - 1.0).abs() < 1e-3, "{dc}");
    }

    #[test]
    fn up_and_down_again_passes_the_audio_band_untouched() {
        let mut up = Up2::new(Halfband::new(PAIRS, BETA));
        let mut down = Down2::new(Halfband::new(PAIRS, BETA));
        for hz in [100.0f32, 5_000.0, 18_000.0] {
            up.reset();
            down.reset();
            let input = tone(hz, 48_000.0, 9_600);
            let output: Vec<f32> = input.iter().map(|&x| down.process(up.process(x))).collect();
            let gain = rms(&output[4_800..]) / rms(&input[4_800..]);
            assert!((20.0 * gain.log10()).abs() < 0.1, "{hz} Hz: {gain}");
        }
    }

    #[test]
    fn coming_down_removes_what_would_fold_back() {
        // A tone at 70% of the high rate's Nyquist would fold to 30%.
        let mut down = Down2::new(Halfband::new(PAIRS, BETA));
        let high = tone(33_600.0, 96_000.0, 19_200);
        let output: Vec<f32> = high
            .chunks(2)
            .map(|pair| down.process([pair[0], pair[1]]))
            .collect();
        let level = 20.0 * (rms(&output[4_800..]) / rms(&high)).log10();
        assert!(level < -60.0, "{level} dB");
    }
}
