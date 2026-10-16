//! The pYIN pitch tracker[^paper], for `f64` signals.
//!
//! Ported from the `pyin` crate 1.2.0 by Jeongmin Liu (MIT licence, below),
//! itself a translation of librosa 0.9.1's `pyin` (ISC licence, below). The
//! port does the crate's arithmetic in the crate's order, sums included, so it
//! gives the crate's results bit for bit (`tests/golden.txt` holds them). It
//! leaves out what kazoo never used: the C interface, `f32`, reflect padding,
//! the timestamps and the setters. The crate is no longer a dependency because
//! it hard-depends on `ndarray-npy`, whose `zip` 0.5 is yanked, so
//! `cargo install kzoo` could not resolve without `--locked`.
//!
//! Where the crate panicked, the port does not:
//! - settings it cannot hold, or a grid past [`MAX_STATES`], are refused;
//! - a signal shorter than a frame is refused;
//! - non-finite samples are read as silence;
//! - a candidate pitch below the lowest bin goes to the lowest bin, as
//!   librosa clips it.
//!
//! A candidate above the highest bin counts for nothing, as in the crate.
//!
//! [^paper]: Mauch, Matthias, and Simon Dixon. "pYIN: A fundamental frequency
//!     estimator using probabilistic threshold distributions." ICASSP 2014.
//!
//! ```text
//! MIT License
//!
//! Copyright (c) 2022 Jeongmin Liu
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//! ```
//!
//! ```text
//! ISC License
//!
//! Copyright (c) 2013--2017, librosa development team.
//!
//! Permission to use, copy, modify, and/or distribute this software for any
//! purpose with or without fee is hereby granted, provided that the above
//! copyright notice and this permission notice appear in all copies.
//!
//! THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
//! WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
//! MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
//! ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
//! WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
//! ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
//! OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
//! ```

mod hmm;
mod prior;
mod yin;

use std::borrow::Cow;

use crate::{Error, Result};

/// The most states (a voiced and an unvoiced one per pitch bin) a tracker
/// may run.
///
/// Decoding grows with the states: each frame keeps a pointer for every
/// state and sorts them. The wall listener uses 0.1 semitone bins over its
/// range, while the voice judge uses coarser bins to rule only on octave and
/// voicing. The ceiling is where the crate's dense table of `f64`
/// transitions, states squared, reached the 64 MiB kazoo allowed it, so the
/// configurations accepted did not change when that table went: it leaves
/// room for 0.1 over the full 50–4000 Hz voice range (1518 states) and
/// refuses 0.05 over it (3036).
pub const MAX_STATES: usize = 2896;

/// The finest bins a tracker may use, in semitones: a tenth of a cent.
const MIN_RESOLUTION: f64 = 0.001;

/// Where the first frame starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Framing {
    /// The signal's first sample is the first frame's centre: half a frame of
    /// this value goes on each side.
    Center(f64),
    /// The signal's first sample is the first frame's first: no padding.
    Valid,
}

/// What a tracker is built for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The lowest pitch searched, in Hz.
    pub fmin: f64,
    /// The highest pitch searched, in Hz; at most half the rate.
    pub fmax: f64,
    /// The signal's sample rate.
    pub rate: u32,
    /// Samples in a frame.
    pub frame_length: usize,
    /// Samples the autocorrelation window spans; under `frame_length`.
    pub win_length: usize,
    /// Samples between frames.
    pub hop_length: usize,
    /// Semitones per pitch bin, from 0.001 to under 1; 0.01 is cents.
    pub resolution: f64,
}

impl Settings {
    /// pYIN's own defaults for a range, rate and frame: the window half the
    /// frame, the hop a quarter of it, and bins a tenth of a semitone.
    #[must_use]
    pub const fn plain(fmin: f64, fmax: f64, rate: u32, frame_length: usize) -> Self {
        Self {
            fmin,
            fmax,
            rate,
            frame_length,
            win_length: frame_length / 2,
            hop_length: frame_length / 4,
            resolution: 0.1,
        }
    }

    /// The periods, pitch bins and transition band these settings make,
    /// or why a tracker cannot be built for them.
    pub fn grid(&self) -> Result<Grid> {
        let rate = f64::from(self.rate);
        if !(self.fmin.is_finite() && self.fmax.is_finite() && 0.0 < self.fmin) {
            return refuse(format!(
                "pyin needs a finite, positive fmin, not {}",
                self.fmin
            ));
        }
        if !(self.fmin < self.fmax && self.fmax <= rate / 2.0) {
            return refuse(format!(
                "pyin needs fmin < fmax <= half the rate: {} to {} Hz at {rate} Hz",
                self.fmin, self.fmax
            ));
        }
        if self.win_length == 0 || self.win_length >= self.frame_length {
            return refuse(format!(
                "pyin needs a window of 1 to {} samples for a frame of {}, not {}",
                self.frame_length.saturating_sub(1),
                self.frame_length,
                self.win_length
            ));
        }
        if self.hop_length == 0 {
            return refuse("pyin needs a hop of at least one sample".into());
        }
        if !(MIN_RESOLUTION <= self.resolution && self.resolution < 1.0) {
            return refuse(format!(
                "pyin needs a resolution from {MIN_RESOLUTION} to under 1 semitone, not {}",
                self.resolution
            ));
        }

        // Worked in floats first: a tiny fmin or resolution makes values past
        // any usize, and a cast would saturate. Every value cast below is
        // already known to be small.
        let longest = (self.frame_length - self.win_length - 1) as f64;
        let min_period = (rate / self.fmax).floor().max(1.0);
        let max_period = (rate / self.fmin).ceil().min(longest);
        if max_period < min_period + 2.0 {
            return refuse(format!(
                "frame_length {} is too short for {} Hz at {rate} Hz: pyin needs \
                 periods {min_period} to {max_period} to span at least 2 samples",
                self.frame_length, self.fmin
            ));
        }

        let bins_per_semitone = (1.0 / self.resolution).ceil();
        let pitch_bins = (12.0 * bins_per_semitone * (self.fmax / self.fmin).log2()).floor() + 1.0;
        if pitch_bins < 2.0 {
            return refuse(format!(
                "range [{}, {}] Hz holds fewer than 2 pyin pitch bins",
                self.fmin, self.fmax
            ));
        }
        if 2.0 * pitch_bins > MAX_STATES as f64 {
            return refuse(format!(
                "resolution {} over [{}, {}] Hz makes {} pyin states, more than the \
                 {MAX_STATES} a tracker may run",
                self.resolution,
                self.fmin,
                self.fmax,
                2.0 * pitch_bins
            ));
        }

        let semitones_per_hop =
            (hmm::MAX_TRANSITION_RATE * 12.0 * self.hop_length as f64 / rate).round();
        let transition_width = semitones_per_hop.mul_add(bins_per_semitone, 1.0);
        if transition_width >= pitch_bins {
            return refuse(format!(
                "hop_length {} at {rate} Hz lets a pitch move {transition_width} bins \
                 per hop, as many as the range's {pitch_bins} bins or more",
                self.hop_length
            ));
        }

        Ok(Grid {
            min_period: min_period as usize,
            max_period: max_period as usize,
            bins_per_semitone: bins_per_semitone as usize,
            pitch_bins: pitch_bins as usize,
            transition_width: transition_width as usize,
        })
    }
}

/// The search grid a tracker's settings make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    /// The shortest period searched, in samples.
    pub min_period: usize,
    /// The longest period searched, in samples.
    pub max_period: usize,
    /// Pitch bins per semitone.
    pub bins_per_semitone: usize,
    /// Pitch bins across the range; the tracker has twice as many states.
    pub pitch_bins: usize,
    /// Bins a pitch may move in one hop, plus one.
    pub transition_width: usize,
}

impl Grid {
    /// The tracker's states: a voiced and an unvoiced one per pitch bin.
    #[must_use]
    pub const fn states(&self) -> usize {
        2 * self.pitch_bins
    }

    const fn periods(&self) -> usize {
        self.max_period - self.min_period + 1
    }
}

/// One estimate per frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// The pitch in Hz, or the fill value where the frame is unvoiced.
    pub f0: Vec<f64>,
    /// Whether the decoded path is voiced at each frame.
    pub voiced: Vec<bool>,
    /// The probability each frame is voiced.
    pub voiced_prob: Vec<f64>,
}

/// A pYIN tracker: its FFT plans, priors and transitions, built once and
/// reused for every signal it reads.
pub struct Pyin {
    settings: Settings,
    grid: Grid,
    yin: yin::Yin,
    hmm: hmm::Hmm,
}

impl std::fmt::Debug for Pyin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pyin")
            .field("settings", &self.settings)
            .field("grid", &self.grid)
            .finish_non_exhaustive()
    }
}

impl Pyin {
    /// Build a tracker, or say why these settings cannot make one.
    pub fn new(settings: Settings) -> Result<Self> {
        let grid = settings.grid()?;
        Ok(Self {
            settings,
            grid,
            yin: yin::Yin::new(&settings, &grid),
            hmm: hmm::Hmm::new(&grid),
        })
    }

    /// Track a signal's pitch.
    ///
    /// Non-finite samples, and a non-finite padding value, are read as
    /// silence. `fill_unvoiced` stands in the
    /// pitch of every unvoiced frame (typically `0.0` or NaN).
    ///
    /// # Errors
    ///
    /// A signal too short for one frame.
    pub fn track(&mut self, wav: &[f64], fill_unvoiced: f64, framing: Framing) -> Result<Track> {
        let wav = self.framed(wav, framing);
        let frame_length = self.settings.frame_length;
        if wav.len() < frame_length {
            return refuse(format!(
                "pyin needs at least one frame of {frame_length} samples, not {}",
                wav.len()
            ));
        }
        let frames = (wav.len() - frame_length) / self.settings.hop_length + 1;
        let periods = self.grid.periods();

        let mut observed = vec![0.0; frames * 2 * self.grid.pitch_bins];
        let mut voiced_prob = Vec::with_capacity(frames);
        let mut yin = vec![0.0; periods];
        let mut shifts = vec![0.0; periods];
        let mut probs = vec![0.0; periods];
        for (index, observed) in observed
            .chunks_exact_mut(2 * self.grid.pitch_bins)
            .enumerate()
        {
            let start = index * self.settings.hop_length;
            self.yin
                .difference(&wav[start..start + frame_length], &self.grid, &mut yin)?;
            yin::parabolic_shifts(&yin, &mut shifts);
            prior::trough_probs(&yin, &mut probs);
            voiced_prob.push(self.observe(&probs, &shifts, observed));
        }

        let states = self.hmm.decode(&observed, frames);
        let pitch_bins = self.grid.pitch_bins;
        let steps = (12 * self.grid.bins_per_semitone) as f64;
        let f0 = states
            .iter()
            .map(|&state| {
                if state < pitch_bins {
                    self.settings.fmin * ((state as f64) / steps).exp2()
                } else {
                    fill_unvoiced
                }
            })
            .collect();
        let voiced = states.iter().map(|&state| state < pitch_bins).collect();
        Ok(Track {
            f0,
            voiced,
            voiced_prob,
        })
    }

    /// The signal as the frames read it: padded if centred, with
    /// non-finite samples and padding made silent.
    fn framed<'a>(&self, wav: &'a [f64], framing: Framing) -> Cow<'a, [f64]> {
        let finite = if wav.iter().all(|x| x.is_finite()) {
            Cow::Borrowed(wav)
        } else {
            Cow::Owned(
                wav.iter()
                    .map(|&x| if x.is_finite() { x } else { 0.0 })
                    .collect(),
            )
        };
        let Framing::Center(value) = framing else {
            return finite;
        };
        let value = if value.is_finite() { value } else { 0.0 };
        let half = self.settings.frame_length / 2;
        let mut padded = Vec::with_capacity(finite.len() + 2 * half);
        padded.resize(half, value);
        padded.extend_from_slice(&finite);
        padded.resize(finite.len() + 2 * half, value);
        Cow::Owned(padded)
    }

    /// One frame's observation probabilities from its trough probabilities:
    /// each trough's pitch, refined between periods, puts its probability in
    /// its pitch bin, and the unvoiced states share what is left. Returns the
    /// probability the frame is voiced.
    fn observe(&self, probs: &[f64], shifts: &[f64], observed: &mut [f64]) -> f64 {
        let pitch_bins = self.grid.pitch_bins;
        let steps = (12 * self.grid.bins_per_semitone) as f64;
        let rate = f64::from(self.settings.rate);
        for (period, (&prob, &shift)) in probs.iter().zip(shifts).enumerate() {
            if prob == 0.0 {
                continue;
            }
            let f0 = rate / ((period + self.grid.min_period) as f64 + shift);
            let bin = ((f0 / self.settings.fmin).log2() * steps).round();
            // A pitch above the top bin (an infinite one included) fell in
            // the unvoiced half in the crate, which the next step overwrites:
            // it counts for nothing.
            if !bin.is_finite() {
                continue;
            }
            // One below the bottom bin panicked in the crate; librosa clips
            // it to the bottom bin.
            let bin = bin.max(0.0) as usize;
            if bin < pitch_bins {
                // A later trough in the same bin replaces an earlier one.
                observed[bin] = prob;
            }
        }
        // In order, as the crate summed down its columns; NaN reads as
        // nothing voiced, as the crate's `max(0).min(1)` made it.
        let total: f64 = observed[..pitch_bins].iter().sum();
        let voiced = if total.is_nan() {
            0.0
        } else {
            total.clamp(0.0, 1.0)
        };
        let unvoiced = (1.0 - voiced) / pitch_bins as f64;
        observed[pitch_bins..].fill(unvoiced);
        voiced
    }
}

const fn refuse<T>(message: String) -> Result<T> {
    Err(Error::Config(message))
}

#[cfg(test)]
mod tests;
