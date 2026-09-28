//! The desk's metronome: a click on every beat while the studio plays.
//!
//! It clicks from the song position the song clock gives it each frame (see
//! [`crate::song`]), so it lands on exactly the frames the instruments'
//! beats do, whatever the buffer size, and keeps counting while muted so
//! switching the click on mid-song lands on the beat. Beat one of each bar is
//! accented. Real-time safe: no allocation, all state inline.

/// Beats in a bar (4/4).
pub const BEATS_PER_BAR: u32 = 4;

/// Click level on beat one.
const ACCENT_LEVEL: f32 = 0.28;

/// Click level on other beats.
const BEAT_LEVEL: f32 = 0.16;

/// Click pitch on beat one, in Hz.
const ACCENT_HZ: f32 = 1_760.0;

/// Click pitch on other beats, in Hz.
const BEAT_HZ: f32 = 1_320.0;

/// Click length.
const CLICK_SECONDS: f32 = 0.03;

/// Decay time constant of the click.
const DECAY_SECONDS: f32 = 0.006;

/// A metronome following the song position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metronome {
    sample_rate: f32,
    audible: bool,
    /// Song position on the previous frame, while playing.
    previous: Option<f64>,
    /// Beat of the last click, while playing.
    last_beat: Option<u64>,
    /// Current click: remaining frames, amplitude, phase and step.
    remaining: u32,
    amplitude: f32,
    phase: f32,
    step: f32,
    decay: f32,
    click_frames: u32,
}

impl Metronome {
    /// A silent metronome at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            sample_rate,
            audible: false,
            previous: None,
            last_beat: None,
            remaining: 0,
            amplitude: 0.0,
            phase: 0.0,
            step: 0.0,
            decay: (-1.0 / (DECAY_SECONDS * sample_rate)).exp(),
            click_frames: (CLICK_SECONDS * sample_rate).round() as u32,
        }
    }

    /// Switch the click on or off; it keeps time either way.
    pub const fn set_audible(&mut self, audible: bool) {
        self.audible = audible;
    }

    /// The beat within the bar (0 = beat one) the metronome last clicked, or
    /// `None` while stopped.
    #[must_use]
    pub fn beat_in_bar(&self) -> Option<u32> {
        self.last_beat
            .map(|beat| (beat % u64::from(BEATS_PER_BAR)) as u32)
    }

    /// Advance one frame at song position `beat` (`None` while stopped),
    /// with `beats_per_frame` the distance one frame covers at the current
    /// tempo, and return the frame's output sample.
    ///
    /// A beat clicks on the first frame at or past it: the frame where the
    /// song starts on beat 0 clicks, as does every frame that crosses a
    /// whole beat.
    pub fn tick(&mut self, beat: Option<f64>, beats_per_frame: f64) -> f32 {
        let Some(beat) = beat.filter(|beat| beat.is_finite() && *beat >= 0.0) else {
            self.previous = None;
            self.last_beat = None;
            self.remaining = 0;
            return 0.0;
        };
        let previous = self
            .previous
            .unwrap_or_else(|| beat - beats_per_frame.max(0.0));
        self.previous = Some(beat);
        // The whole beat reached on this frame, if the song passed one since
        // the previous frame. Song positions are small and non-negative:
        // the conversion is exact.
        let reached = beat.floor();
        if reached > previous {
            let whole = reached as u64;
            self.last_beat = Some(whole);
            self.start_click(whole % u64::from(BEATS_PER_BAR) == 0);
        }

        if self.remaining == 0 {
            return 0.0;
        }
        self.remaining -= 1;
        let sample = self.phase.sin() * self.amplitude;
        self.phase = (self.phase + self.step).rem_euclid(std::f32::consts::TAU);
        self.amplitude *= self.decay;
        if self.audible { sample } else { 0.0 }
    }

    fn start_click(&mut self, accent: bool) {
        let (level, hz) = if accent {
            (ACCENT_LEVEL, ACCENT_HZ)
        } else {
            (BEAT_LEVEL, BEAT_HZ)
        };
        self.remaining = self.click_frames;
        self.amplitude = level;
        self.phase = 0.0;
        self.step = std::f32::consts::TAU * hz / self.sample_rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// Run the metronome over `frames` of a song playing at `bpm` from beat
    /// `start`, returning the frames on which clicks start.
    fn click_starts(metronome: &mut Metronome, bpm: f64, start: f64, frames: usize) -> Vec<usize> {
        let per_frame = kazoo_core::ipc::follow::beats_in(1.0, bpm, f64::from(RATE));
        let mut starts = Vec::new();
        for frame in 0..frames {
            let beat =
                start + kazoo_core::ipc::follow::beats_in(frame as f64, bpm, f64::from(RATE));
            metronome.tick(Some(beat), per_frame);
            if metronome.remaining + 1 == metronome.click_frames {
                starts.push(frame);
            }
        }
        starts
    }

    #[test]
    fn clicks_land_on_every_beat_at_the_tempo() {
        let mut metronome = Metronome::new(RATE);
        metronome.set_audible(true);
        // 120 BPM at 48 kHz: a beat every 24 000 frames.
        let starts = click_starts(&mut metronome, 120.0, 0.0, 100_000);
        assert_eq!(starts, vec![0, 24_000, 48_000, 72_000, 96_000]);
    }

    #[test]
    fn beat_one_is_accented() {
        let mut metronome = Metronome::new(RATE);
        metronome.set_audible(true);
        let per_frame = kazoo_core::ipc::follow::beats_in(1.0, 240.0, f64::from(RATE));
        let mut peaks = [0.0_f32; 5];
        let mut frame = 0_u64;
        for (beat, peak) in peaks.iter_mut().enumerate() {
            for _ in 0..12_000 {
                let song = kazoo_core::ipc::follow::beats_in(frame as f64, 240.0, f64::from(RATE));
                *peak = peak.max(metronome.tick(Some(song), per_frame).abs());
                frame += 1;
            }
            assert_eq!(metronome.beat_in_bar(), Some(beat as u32 % BEATS_PER_BAR));
        }
        assert!(peaks[0] > peaks[1] * 1.4, "{peaks:?}");
        assert!(peaks[4] > peaks[3] * 1.4, "{peaks:?}");
    }

    #[test]
    fn a_muted_metronome_keeps_time() {
        let mut metronome = Metronome::new(RATE);
        let per_frame = kazoo_core::ipc::follow::beats_in(1.0, 120.0, f64::from(RATE));
        for frame in 0..30_000 {
            let song = kazoo_core::ipc::follow::beats_in(f64::from(frame), 120.0, f64::from(RATE));
            assert!(metronome.tick(Some(song), per_frame).abs() < f32::EPSILON);
        }
        // Unmuted mid-beat: silent until the next beat, then on time.
        metronome.set_audible(true);
        let starts = click_starts(&mut metronome, 120.0, 1.25, 20_000);
        assert_eq!(starts, vec![18_000]);
    }

    #[test]
    fn stopping_silences_and_a_song_joined_mid_beat_waits_for_the_next() {
        let mut metronome = Metronome::new(RATE);
        metronome.set_audible(true);
        click_starts(&mut metronome, 120.0, 0.0, 30_000);
        assert!(metronome.tick(None, 0.0).abs() < f32::EPSILON);
        assert_eq!(metronome.beat_in_bar(), None);
        // Playing from beat 2.5: the next click is beat 3, half a beat on.
        assert_eq!(
            click_starts(&mut metronome, 120.0, 2.5, 30_000),
            vec![12_000]
        );
        assert_eq!(metronome.beat_in_bar(), Some(3));
    }

    #[test]
    fn nonsense_positions_are_silent() {
        let mut metronome = Metronome::new(RATE);
        metronome.set_audible(true);
        assert!(metronome.tick(Some(f64::NAN), 0.001).abs() < f32::EPSILON);
        assert!(metronome.tick(Some(-1.0), 0.001).abs() < f32::EPSILON);
        let mut garbage = Metronome::new(f32::NAN);
        garbage.set_audible(true);
        assert!(garbage.tick(Some(0.0), 0.001).is_finite());
    }
}
