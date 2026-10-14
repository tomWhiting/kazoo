use super::TrackSettings;
use super::VOICE_HOP;

#[cfg(test)]
mod tests;

/// Open above −50 dBFS with a voiced pitch, well above the noise floor.
pub const GATE_ON_DB: f32 = -50.0;
/// Six dB of hysteresis exceeds the fast envelope ripple at 50 Hz.
pub const GATE_OFF_DB: f32 = -56.0;
/// Twelve milliseconds bridges a glottal break without an audible release hold.
pub const GATE_OFF_HOLD_MS: u64 = 12;
/// Syllables dip 10–30 dB; tremolo and vibrato move only 3–6 dB.
pub const ONSET_RISE_DB: f32 = 9.0;
/// Spectral flux permits a shallow consonant dip but needs a level rise.
pub const ONSET_FLUX_RISE_DB: f32 = 4.0;
/// Flux arrives about one judge hop late; both pieces must be within 30 ms.
pub const ONSET_FLUX_WINDOW_MS: u64 = 30;
/// Sixty milliseconds prevents duplicate onsets within a fast syllable.
pub const ONSET_REFRACTORY_MS: u64 = 60;

#[derive(Debug, Clone, Copy)]
struct Pending {
    hz: f32,
    at: u64,
    steps: u8,
}

#[derive(Debug)]
pub(super) struct Gate {
    settings: TrackSettings,
    pitch: [Option<f32>; 2],
    high: bool,
    off_frames: u64,
    hold_frames: u64,
    refractory: u64,
    flux_window: u64,
    last_onset: Option<u64>,
    flux: Option<u64>,
    rise: Option<u64>,
    rise_active: bool,
    pending: Option<Pending>,
    published: Option<f32>,
    onset_at: u64,
    amplitude_window: u64,
}

impl Gate {
    #[cfg(test)]
    pub(super) fn new(rate: u32) -> Self {
        Self::with_settings(rate, TrackSettings::VOICE)
    }
    pub(super) fn with_settings(rate: u32, settings: TrackSettings) -> Self {
        let rate = u64::from(rate.max(1));
        Self {
            settings,
            pitch: [None; 2],
            high: false,
            off_frames: 0,
            hold_frames: (GATE_OFF_HOLD_MS * rate).div_ceil(1000 * VOICE_HOP as u64),
            refractory: ONSET_REFRACTORY_MS * rate / 1000,
            flux_window: ONSET_FLUX_WINDOW_MS * rate / 1000,
            last_onset: None,
            flux: None,
            rise: None,
            rise_active: false,
            pending: None,
            published: None,
            onset_at: 0,
            amplitude_window: 10 * rate / 1000,
        }
    }
    pub(super) const fn flux(&mut self, at: u64) {
        self.flux = Some(at);
    }
    #[cfg(test)]
    pub(super) fn frame(&mut self, at: u64, voiced: bool, db: f32, valley_db: f32) -> (bool, bool) {
        self.frame_with_pitch(at, voiced, db, valley_db, 0.0)
    }
    #[cfg(test)]
    pub(super) fn frame_with_pitch(
        &mut self,
        at: u64,
        voiced: bool,
        db: f32,
        valley_db: f32,
        hz: f32,
    ) -> (bool, bool) {
        self.frame_with_level(at, voiced, db, valley_db, hz, db)
    }

    pub(super) fn frame_with_level(
        &mut self,
        at: u64,
        voiced: bool,
        db: f32,
        valley_db: f32,
        hz: f32,
        periodic_db: f32,
    ) -> (bool, bool) {
        let was_high = self.high;
        if self.high {
            if !voiced || db < self.settings.gate_off_db {
                self.off_frames = self.off_frames.saturating_add(1);
                if self.off_frames >= self.hold_frames {
                    self.high = false;
                }
            } else {
                self.off_frames = 0;
            }
        } else if voiced
            && db >= self.settings.gate_on_db
            && periodic_db >= self.settings.gate_on_db
        {
            self.high = true;
            self.off_frames = 0;
        }
        let rise = db - valley_db;
        let rise_active = rise >= ONSET_FLUX_RISE_DB;
        // A crossing is one recovery; a sustained difference is not another syllable.
        if rise_active && !self.rise_active {
            self.rise = Some(at);
        }
        self.rise_active = rise_active;
        let paired = match (self.flux, self.rise) {
            (Some(flux), Some(rise)) => {
                flux.abs_diff(rise) <= self.flux_window
                    && at.saturating_sub(flux.max(rise)) <= self.flux_window
            }
            _ => false,
        };
        let ready = self
            .last_onset
            .is_none_or(|last| at.saturating_sub(last) >= self.refractory);
        let current = (voiced && hz.is_finite() && hz > 0.0).then_some(hz);
        let amplitude = !was_high || rise >= self.settings.onset_rise_db || paired;
        let jump = match (self.settings.jump_onset_cents, current, self.published) {
            (Some(cents), Some(now), Some(before)) => {
                self.pitch[0].is_some() && (1200.0 * (now / before).log2()).abs() > cents
            }
            _ => false,
        };
        self.pitch = [self.pitch[1], current];
        let mut confirmed = None;
        if let Some(mut pending) = self.pending.take() {
            pending.steps += 1;
            let stable =
                current.is_some_and(|hz| (1200.0 * (hz / pending.hz).log2()).abs() <= 30.0);
            let attacked = self
                .flux
                .is_some_and(|at| at.abs_diff(pending.at) <= self.amplitude_window)
                || (amplitude && at.abs_diff(pending.at) <= self.amplitude_window);
            if self.high && (stable || attacked) && pending.steps <= 2 {
                confirmed = Some(pending.at);
                self.published = current.or(Some(pending.hz));
            } else if pending.steps < 2 {
                self.pending = Some(pending);
            }
        }
        if self.high && jump && self.pending.is_none() && confirmed.is_none() {
            if let Some(hz) = current {
                let attacked = amplitude
                    || self
                        .flux
                        .is_some_and(|stamp| stamp.abs_diff(at) <= self.amplitude_window);
                if attacked {
                    confirmed = Some(at);
                    self.published = current;
                } else {
                    self.pending = Some(Pending { hz, at, steps: 0 });
                }
            }
        } else if !jump && self.pending.is_none() {
            self.published = current.or(self.published);
        }
        let onset = self.high && ready && (amplitude || confirmed.is_some());
        if onset {
            self.onset_at = confirmed.unwrap_or(at);
            self.last_onset = Some(self.onset_at);
            self.flux = None;
            self.rise = None;
            // The rest of the rise that made this onset is this onset: the
            // valley is reset part-way up the recovery, and the climb that
            // follows must not be taken for a new ≥ 4 dB rise for the judge's
            // late flux to pair with. A new rise must first fall back under
            // `ONSET_FLUX_RISE_DB`.
            self.rise_active = true;
        }
        (self.high, onset)
    }
    pub(super) const fn published_pitch(&self) -> Option<f32> {
        self.published
    }
    pub(super) const fn onset_at(&self) -> u64 {
        self.onset_at
    }
    pub(super) const fn correct_pitch(&mut self, hz: f32) {
        self.pending = None;
        self.published = Some(hz);
        self.pitch = [Some(hz); 2];
    }
    pub(super) const fn reset(&mut self) {
        self.high = false;
        self.off_frames = 0;
        self.last_onset = None;
        self.flux = None;
        self.rise = None;
        self.rise_active = false;
        self.pitch = [None; 2];
        self.pending = None;
        self.published = None;
        self.onset_at = 0;
    }
}
