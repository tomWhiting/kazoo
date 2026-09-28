//! Listening: how the seats hear the wall.
//!
//! The engine copies the master output (mono) into a ring; a thread takes
//! what has arrived every 250 ms and describes it: level, spectral balance
//! (`kazoo_core`'s spectrum analyser), onset rate, dominant pitch
//! (`kazoo_core`'s pYIN detector), and a short line of plain words ("dark,
//! sparse, slow pulse around A2, quiet").
//!
//! Onsets are found by a rise in short-term energy over an absolute floor,
//! not by `kazoo_core`'s spectral-flux detector: its threshold adapts to the
//! signal, so on a steady drone it fires on rounding noise, and the wall is
//! often a steady drone.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kazoo_core::analysis::{PitchDetector, PitchDetectorConfig, SpectrumAnalyzer};
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use crate::change::utc_now;
use crate::format;
use crate::protocol::Listen;

/// How often the wall is described.
pub const LISTEN_INTERVAL: Duration = Duration::from_millis(250);

/// Silence, in dBFS.
pub const FLOOR_DB: f64 = -120.0;

/// Spectrum frame length.
const SPECTRUM_SIZE: usize = 4_096;

/// Onsets are counted over this many seconds.
const ONSET_WINDOW_SECONDS: f64 = 4.0;

/// Pitch frames need at least this voicing probability to count.
const VOICED: f32 = 0.5;

/// Pitch analysis frame, in samples: one frame is analysed per listening.
const PITCH_FRAME: usize = 4_096;

/// Onset energy frame, in seconds.
const ONSET_FRAME_SECONDS: f32 = 0.01;

/// An onset is a frame this many times louder (in power) than the average of
/// the frames before it...
const ONSET_RISE: f32 = 4.0;

/// ...and louder than this (power, about -50 dBFS RMS)...
const ONSET_FLOOR: f32 = 1.0e-5;

/// ...at least this many frames after the last one.
const ONSET_GAP_FRAMES: u32 = 5;

/// Frames the onset detector averages behind the current one.
const ONSET_HISTORY: usize = 8;

/// Finds onsets as rises in short-term energy.
#[derive(Debug, Clone)]
struct Onsets {
    frame_len: usize,
    sum: f32,
    filled: usize,
    history: [f32; ONSET_HISTORY],
    next: usize,
    since: u32,
}

impl Onsets {
    fn new(sample_rate: f32) -> Self {
        Self {
            // A few hundred samples: the cast is exact.
            frame_len: ((sample_rate * ONSET_FRAME_SECONDS) as usize).max(1),
            sum: 0.0,
            filled: 0,
            history: [0.0; ONSET_HISTORY],
            next: 0,
            since: ONSET_GAP_FRAMES,
        }
    }

    /// Offsets into `samples` where an onset begins.
    fn push(&mut self, samples: &[f32]) -> Vec<usize> {
        let mut found = Vec::new();
        for (offset, sample) in samples.iter().enumerate() {
            let sample = kazoo_core::sanitize_sample(*sample);
            self.sum = sample.mul_add(sample, self.sum);
            self.filled += 1;
            if self.filled < self.frame_len {
                continue;
            }
            // Frame lengths are a few hundred: exact.
            let power = self.sum / self.frame_len as f32;
            let before = self.history.iter().sum::<f32>() / ONSET_HISTORY as f32;
            self.since = self.since.saturating_add(1);
            if power > ONSET_FLOOR && power > before * ONSET_RISE && self.since >= ONSET_GAP_FRAMES
            {
                found.push(offset + 1 - self.frame_len.min(offset + 1));
                self.since = 0;
            }
            self.history[self.next] = power;
            self.next = (self.next + 1) % ONSET_HISTORY;
            self.sum = 0.0;
            self.filled = 0;
        }
        found
    }
}

/// A level in dBFS, with silence at [`FLOOR_DB`].
#[must_use]
pub fn dbfs(level: f64) -> f64 {
    if level.is_finite() && level > 0.0 {
        (20.0 * level.log10()).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

/// Describes the wall from its master output.
pub struct Listener {
    sample_rate: f32,
    spectrum: SpectrumAnalyzer,
    onsets: Onsets,
    pitch: Option<PitchDetector>,
    /// Seconds of audio heard so far.
    heard: f64,
    /// When each recent onset was heard, in seconds of audio.
    onset_times: VecDeque<f64>,
    /// The last spectrum: power per bin, and each bin's frequency.
    power: Vec<f32>,
    bins: Vec<f32>,
    last_pitch: Option<f32>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("sample_rate", &self.sample_rate)
            .field("heard", &self.heard)
            .finish_non_exhaustive()
    }
}

impl Listener {
    /// A listener for audio at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        // 8 kHz to a few hundred kHz: exact in f32.
        let rate = sample_rate.max(8_000) as f32;
        let pitch = PitchDetector::new(PitchDetectorConfig {
            min_frequency: 50.0,
            max_frequency: 1_000.0,
            sample_rate: sample_rate.max(8_000),
            frame_length: PITCH_FRAME,
            voiced_threshold: 0.3,
        });
        Self {
            sample_rate: rate,
            spectrum: SpectrumAnalyzer::new(SPECTRUM_SIZE, rate, 0.0),
            onsets: Onsets::new(rate),
            // Its configuration is fixed and valid for any rate from 8 kHz:
            // without it the wall is still described, with no pitch.
            pitch: match pitch {
                Ok(detector) => Some(detector),
                Err(err) => {
                    eprintln!("kazoo-wall: listening without pitch: {err}");
                    None
                }
            },
            heard: 0.0,
            onset_times: VecDeque::new(),
            power: Vec::new(),
            bins: Vec::new(),
            last_pitch: None,
        }
    }

    /// A listener that does not look for pitch (pYIN is slow unoptimised).
    #[cfg(test)]
    fn without_pitch(sample_rate: u32) -> Self {
        Self {
            pitch: None,
            ..Self::new(sample_rate)
        }
    }

    /// Describe `samples`, the audio heard since the last description.
    pub fn hear(&mut self, samples: &[f32]) -> Listen {
        let mut sum = 0.0_f64;
        let mut peak = 0.0_f32;
        for sample in samples {
            let sample = kazoo_core::sanitize_sample(*sample);
            sum = f64::from(sample).mul_add(f64::from(sample), sum);
            peak = peak.max(sample.abs());
        }
        let rms = if samples.is_empty() {
            0.0
        } else {
            // Sample counts are far below 2^53: exact.
            (sum / samples.len() as f64).sqrt()
        };
        if let Some(frame) = self.spectrum.push_samples(samples) {
            self.power = frame
                .magnitudes_db
                .iter()
                .map(|db| 10.0_f32.powf(db / 10.0))
                .collect();
            self.bins = frame.bin_frequencies.to_vec();
        }
        let start = self.heard;
        for offset in self.onsets.push(samples) {
            // Offsets are within this call's samples: exact enough.
            self.onset_times
                .push_back(start + offset as f64 / f64::from(self.sample_rate));
        }
        self.heard += samples.len() as f64 / f64::from(self.sample_rate);
        while self
            .onset_times
            .front()
            .is_some_and(|at| *at < self.heard - ONSET_WINDOW_SECONDS)
        {
            self.onset_times.pop_front();
        }
        // pYIN is costly: only the latest frame's worth is analysed.
        let tail = &samples[samples.len().saturating_sub(PITCH_FRAME)..];
        if let Some(estimate) = self.pitch.as_mut().and_then(|p| p.push_samples(tail)) {
            self.last_pitch = estimate
                .frequency
                .filter(|_| estimate.voiced_probability >= VOICED);
        }
        self.describe(rms, f64::from(peak))
    }

    fn describe(&self, rms: f64, peak: f64) -> Listen {
        let (centroid, low, mid, high) = self.balance();
        let window = self.heard.clamp(0.25, ONSET_WINDOW_SECONDS);
        // At most a few hundred onsets: exact.
        let onsets_per_second = self.onset_times.len() as f64 / window;
        let rms_db = dbfs(rms);
        let pitch_hz = self.last_pitch.filter(|_| rms_db > -60.0).map(f64::from);
        // The pitch is a finite frequency: the narrowing is exact enough.
        let pitch = pitch_hz.and_then(|hz| format::note_name(hz as f32));
        let mut listen = Listen {
            at: utc_now(),
            rms_db,
            peak_db: dbfs(peak),
            centroid_hz: centroid,
            low,
            mid,
            high,
            onsets_per_second,
            pitch_hz,
            pitch,
            words: String::new(),
        };
        listen.words = words(&listen);
        listen
    }

    /// Spectral centroid and the share of power below 250 Hz, from 250 Hz
    /// to 4 kHz, and above.
    fn balance(&self) -> (f64, f64, f64, f64) {
        let mut total = 0.0_f64;
        let mut weighted = 0.0_f64;
        let mut bands = [0.0_f64; 3];
        for (power, hz) in self.power.iter().zip(&self.bins) {
            let power = f64::from(*power);
            let hz = f64::from(*hz);
            if !power.is_finite() || hz <= 0.0 {
                continue;
            }
            total += power;
            weighted = power.mul_add(hz, weighted);
            let band = if hz < 250.0 {
                0
            } else if hz <= 4_000.0 {
                1
            } else {
                2
            };
            bands[band] += power;
        }
        if total <= 1e-12 {
            return (0.0, 0.0, 0.0, 0.0);
        }
        (
            weighted / total,
            bands[0] / total,
            bands[1] / total,
            bands[2] / total,
        )
    }
}

/// The description as plain words: brightness, density and pulse, the
/// pitch if there is one, and loudness.
#[must_use]
pub fn words(listen: &Listen) -> String {
    let loudness = if listen.rms_db < -60.0 {
        return "silent".to_string();
    } else if listen.rms_db < -40.0 {
        "very quiet"
    } else if listen.rms_db < -28.0 {
        "quiet"
    } else if listen.rms_db < -16.0 {
        "moderate"
    } else {
        "loud"
    };
    let brightness = if listen.centroid_hz < 400.0 {
        "dark"
    } else if listen.centroid_hz < 1_200.0 {
        "warm"
    } else if listen.centroid_hz < 3_000.0 {
        "bright"
    } else {
        "harsh"
    };
    let mut parts = vec![brightness.to_string()];
    if listen.low > 0.6 {
        parts.push("bass-heavy".to_string());
    } else if listen.high > 0.3 {
        parts.push("airy".to_string());
    }
    let rate = listen.onsets_per_second;
    let (density, pulse) = if rate < 0.2 {
        ("sustained", None)
    } else if rate < 1.5 {
        ("sparse", Some("slow pulse"))
    } else if rate < 4.0 {
        ("busy", Some("steady pulse"))
    } else {
        ("dense", Some("fast pulse"))
    };
    parts.push(density.to_string());
    let around = listen
        .pitch
        .as_ref()
        .map(|note| format!(" around {note}"))
        .unwrap_or_default();
    match pulse {
        Some(pulse) => parts.push(format!("{pulse}{around}")),
        None if !around.is_empty() => parts.push(around.trim_start().to_string()),
        None => {}
    }
    parts.push(loudness.to_string());
    parts.join(", ")
}

/// Start the listening thread: every [`LISTEN_INTERVAL`] it takes what the
/// engine sent through `ring` and sends a description to `out`, until
/// `stop` is set or `out` is closed.
///
/// # Errors
///
/// Fails if the thread cannot be started.
pub fn spawn(
    mut ring: HeapCons<f32>,
    sample_rate: u32,
    out: Sender<Listen>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("kazoo-wall-listen".to_string())
        .spawn(move || {
            let mut listener = Listener::new(sample_rate);
            let mut samples = Vec::with_capacity(sample_rate as usize);
            let mut next = Instant::now() + LISTEN_INTERVAL;
            while !stop.load(Ordering::Acquire) {
                let now = Instant::now();
                if now < next {
                    thread::sleep((next - now).min(Duration::from_millis(50)));
                    continue;
                }
                next += LISTEN_INTERVAL;
                samples.clear();
                while let Some(sample) = ring.try_pop() {
                    samples.push(sample);
                }
                if samples.is_empty() {
                    continue;
                }
                if out.send(listener.hear(&samples)).is_err() {
                    // The daemon has stopped listening: so does this thread.
                    return;
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, amplitude: f32, frames: usize, offset: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                amplitude * (std::f32::consts::TAU * hz * (i + offset) as f32 / RATE as f32).sin()
            })
            .collect()
    }

    #[test]
    fn silence_is_silent() {
        let mut listener = Listener::new(RATE);
        let listen = listener.hear(&vec![0.0; 12_000]);
        assert_eq!(listen.words, "silent");
        assert!((listen.rms_db - FLOOR_DB).abs() < f64::EPSILON);
        assert!(listen.pitch.is_none());
        let listen = listener.hear(&[]);
        assert_eq!(listen.words, "silent");
    }

    #[test]
    fn a_low_steady_tone_is_dark_sustained_and_pitched() {
        let mut listener = Listener::new(RATE);
        // Long enough for the tone's own start to leave the onset window.
        listener.hear(&sine(110.0, 0.03, 216_000, 0));
        let listen = listener.hear(&sine(110.0, 0.03, 12_000, 216_000));
        assert!(
            (listen.rms_db - dbfs(0.03 / 2.0_f64.sqrt())).abs() < 0.5,
            "{listen:?}"
        );
        assert!(listen.centroid_hz < 400.0, "{listen:?}");
        assert!(listen.low > 0.8, "{listen:?}");
        assert_eq!(listen.pitch.as_deref(), Some("A2"), "{listen:?}");
        assert!(listen.onsets_per_second < 0.3, "{listen:?}");
        assert!(listen.words.starts_with("dark"), "{}", listen.words);
        assert!(listen.words.contains("around A2"), "{}", listen.words);
        assert!(listen.words.ends_with("quiet"), "{}", listen.words);
        assert!(listen.words.contains("sustained"), "{}", listen.words);
    }

    #[test]
    fn clicks_count_as_onsets() {
        let mut listener = Listener::without_pitch(RATE);
        let mut listen = listener.hear(&[]);
        let mut frame = 0;
        for _ in 0..16 {
            let block: Vec<f32> = (0..12_000)
                .map(|i| {
                    let at = frame + i;
                    // Two noisy bursts a second.
                    if at % 24_000 < 400 {
                        (((at * 7_919) % 1_000) as f32 / 500.0 - 1.0) * 0.8
                    } else {
                        0.0
                    }
                })
                .collect();
            frame += 12_000;
            listen = listener.hear(&block);
        }
        assert!(
            (1.5..=2.5).contains(&listen.onsets_per_second),
            "{listen:?}"
        );
    }

    #[test]
    fn words_cover_the_range() {
        let base = Listen {
            at: String::new(),
            rms_db: -20.0,
            peak_db: -10.0,
            centroid_hz: 5_000.0,
            low: 0.1,
            mid: 0.4,
            high: 0.5,
            onsets_per_second: 6.0,
            pitch_hz: None,
            pitch: None,
            words: String::new(),
        };
        assert_eq!(words(&base), "harsh, airy, dense, fast pulse, moderate");
        let quiet = Listen {
            rms_db: -35.0,
            centroid_hz: 300.0,
            low: 0.7,
            onsets_per_second: 0.5,
            pitch: Some("A2".to_string()),
            ..base.clone()
        };
        assert_eq!(
            words(&quiet),
            "dark, bass-heavy, sparse, slow pulse around A2, quiet"
        );
        let drone = Listen {
            rms_db: -10.0,
            centroid_hz: 800.0,
            high: 0.1,
            onsets_per_second: 0.0,
            pitch: Some("C3".to_string()),
            ..base
        };
        assert_eq!(words(&drone), "warm, sustained, around C3, loud");
    }

    #[test]
    fn the_thread_describes_what_arrives_and_stops() {
        use ringbuf::traits::{Producer, Split};
        let (mut tx, rx) = ringbuf::HeapRb::<f32>::new(RATE as usize).split();
        let (out, described) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(rx, RATE, out, Arc::clone(&stop)).unwrap();
        tx.push_slice(&sine(440.0, 0.5, 12_000, 0));
        let listen = described.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(listen.rms_db > -10.0, "{listen:?}");
        stop.store(true, Ordering::Release);
        handle.join().unwrap();
    }
}
