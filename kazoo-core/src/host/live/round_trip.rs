use super::CLIP_RAW;

/// The measured round trip through a loopback cable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundTripResult {
    /// The median of the 8 arrivals, in output frames from the click's write
    /// to its arrival in a read: both buffers, the ring and the device.
    pub frames: u32,
    /// The latest arrival less the earliest, in frames.
    pub spread: u32,
    /// `frames` in milliseconds at the measured rate.
    pub ms: f32,
}

/// Why the round trip gave no figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundTripError {
    /// At least one click never rose above the noise within 0.25 s.
    NothingCameBack,
    /// The arrivals spread over more than 1 ms.
    TooUneven { spread: u32 },
    /// The input reached full scale while a click was expected.
    Clipped,
}

/// Playback and capture positions share an output-frame clock. Call play and
/// hear for the same output block, including blocks spent priming the input.
#[derive(Debug)]
pub struct RoundTrip {
    rate: u32,
    played: u64,
    heard: u64,
    noise_square: f64,
    noise_count: u64,
    window: Box<[f32]>,
    arrivals: [u32; 8],
    completed: usize,
    missing: bool,
    clipped: bool,
    result: Option<Result<RoundTripResult, RoundTripError>>,
}

impl RoundTrip {
    /// Allocate the detection window before starting either callback.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        assert!(rate >= 4);
        Self {
            rate,
            played: 0,
            heard: 0,
            noise_square: 0.0,
            noise_count: 0,
            window: vec![0.0; (rate / 4) as usize].into_boxed_slice(),
            arrivals: [0; 8],
            completed: 0,
            missing: false,
            clipped: false,
            result: None,
        }
    }

    /// The output callback's half: 0.5 s of quiet, then 8 one-sample clicks
    /// at 0.25, 0.5 s apart, then quiet.
    pub fn play(&mut self, out: &mut [f32]) {
        let spacing = u64::from(self.rate) / 2;
        for sample in out {
            let click = self.played >= spacing
                && self.played <= spacing * 8
                && self.played.is_multiple_of(spacing);
            *sample = if click { 0.25 } else { 0.0 };
            self.played += 1;
        }
    }

    /// Feed the same block's input, as a `LiveInput` read gave it.
    pub fn hear(&mut self, input: &[f32]) {
        let spacing = u64::from(self.rate) / 2;
        for &raw in input {
            let sample = if raw.is_finite() { raw } else { 0.0 };
            if self.heard < spacing {
                self.noise_square += f64::from(sample).powi(2);
                self.noise_count += 1;
            } else if self.completed < 8 {
                let start = spacing * (self.completed as u64 + 1);
                let offset = self.heard.saturating_sub(start);
                if self.heard >= start && offset < self.window.len() as u64 {
                    if sample.abs() >= CLIP_RAW || !raw.is_finite() {
                        self.clipped = true;
                    }
                    if let Some(target) = self.window.get_mut(offset as usize) {
                        *target = sample;
                    }
                    if offset + 1 == self.window.len() as u64 {
                        self.finish_window();
                    }
                }
            }
            self.heard += 1;
        }
    }

    fn finish_window(&mut self) {
        let maximum = self
            .window
            .iter()
            .fold(0.0_f32, |peak, x| peak.max(x.abs()));
        let noise = (self.noise_square / self.noise_count.max(1) as f64).sqrt();
        let arrival = self
            .window
            .iter()
            .position(|x| x.abs() > maximum * 0.5 && f64::from(x.abs()) > 10.0 * noise);
        if let Some(offset) = arrival {
            if let Some(target) = self.arrivals.get_mut(self.completed) {
                *target = offset as u32;
            }
        } else {
            self.missing = true;
        }
        self.completed += 1;
        if self.completed == 8 {
            self.finish();
        }
    }

    fn finish(&mut self) {
        self.arrivals.sort_unstable();
        let spread = self.arrivals[7] - self.arrivals[0];
        self.result = Some(if self.clipped {
            Err(RoundTripError::Clipped)
        } else if self.missing {
            Err(RoundTripError::NothingCameBack)
        } else if spread > self.rate / 1000 {
            Err(RoundTripError::TooUneven { spread })
        } else {
            let frames = u32::midpoint(self.arrivals[3], self.arrivals[4]);
            Ok(RoundTripResult {
                frames,
                spread,
                ms: frames as f32 * 1000.0 / self.rate as f32,
            })
        });
    }

    /// None until the last click's window has been heard.
    #[must_use]
    pub const fn result(&self) -> Option<Result<RoundTripResult, RoundTripError>> {
        self.result
    }
}
