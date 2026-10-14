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
    fresh_reading: bool,
    jump_window: u64,
    amplitude_at: Option<u64>,
    attack_at: Option<u64>,
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
            fresh_reading: false,
            jump_window: 2 * settings.judge_config(rate as u32).hop_length as u64,
            amplitude_at: None,
            attack_at: None,
        }
    }
    pub(super) fn set(&mut self, settings: super::GateSettings, rate: u32) {
        let settings = settings.held();
        self.settings.gate_on_db = settings.open_db;
        self.settings.gate_off_db = settings.close_db;
        self.hold_frames =
            (settings.hold_ms * rate as f32 / (1000.0 * VOICE_HOP as f32)).ceil() as u64;
        self.off_frames = 0;
    }

    pub(super) const fn flux(&mut self, at: u64) {
        self.flux = Some(at);
    }
    pub(super) const fn reading(&mut self) {
        self.fresh_reading = true;
    }
    pub(super) const fn attack(&mut self, at: u64) {
        self.attack_at = Some(at);
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
        self.reading();
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
        let reading = std::mem::replace(&mut self.fresh_reading, false);
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
        let amplitude = self.amplitude(at, db, valley_db, was_high);
        let ready = self
            .last_onset
            .is_none_or(|last| at.saturating_sub(last) >= self.refractory);
        let current = (voiced && hz.is_finite() && hz > 0.0).then_some(hz);
        if self.settings.jump_onset_cents.is_some() {
            return self.instrument_frame(at, was_high, ready, current);
        }
        let jump = match (self.settings.jump_onset_cents, current, self.published) {
            (Some(cents), Some(now), Some(before)) => {
                self.pitch[0].is_some() && (1200.0 * (now / before).log2()).abs() > cents
            }
            _ => false,
        };
        self.pitch = [self.pitch[1], current];
        let mut confirmed = None;
        if let Some(mut pending) = self.pending.take() {
            pending.steps += u8::from(reading);
            let stable = reading
                && current.is_some_and(|hz| (1200.0 * (hz / pending.hz).log2()).abs() <= 30.0);
            let attacked = self.amplitude_near(pending.at)
                || (amplitude && at.abs_diff(pending.at) <= self.amplitude_window);
            let timely = at.saturating_sub(pending.at) <= self.jump_window;
            if self.high && timely && (stable || attacked) && pending.steps <= 2 {
                confirmed = Some(pending.at);
                self.published = current.or(Some(pending.hz));
            } else if timely && pending.steps < 2 {
                self.pending = Some(pending);
            }
        }
        if self.high && reading && jump && self.pending.is_none() && confirmed.is_none() {
            if let Some(hz) = current {
                let attacked = amplitude || self.amplitude_near(at);
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
        #[cfg(test)]
        if crate::analysis::cycle::diagnostics::enabled() && (jump || onset || confirmed.is_some())
        {
            println!(
                "gate at={at} hz={hz:.4} published={:?} jump={jump} amplitude={amplitude} ready={ready} pending={:?} confirmed={confirmed:?} onset={onset}",
                self.published, self.pending
            );
        }
        if onset {
            if amplitude {
                self.amplitude_at = Some(at);
            }
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

    fn amplitude(&mut self, at: u64, db: f32, valley_db: f32, was_high: bool) -> bool {
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
        !was_high || rise >= self.settings.onset_rise_db || paired
    }

    fn instrument_frame(
        &mut self,
        at: u64,
        was_high: bool,
        ready: bool,
        current: Option<f32>,
    ) -> (bool, bool) {
        // Pitch stays continuous through an unpicked fretted change. A pick
        // already articulates the string even when its pitch has not moved.
        self.published = current.or(self.published);
        let attack = self
            .attack_at
            .filter(|&stamp| stamp <= at && at - stamp <= self.amplitude_window);
        let onset = self.high
            && ready
            && (attack.is_some_and(|stamp| stamp == at || !was_high)
                || (!was_high && self.last_onset.is_none()));
        if onset {
            // Acquisition may take longer than the jump-pairing window. The
            // first pick still starts the note and its refractory interval.
            self.onset_at = if self.last_onset.is_none() {
                self.attack_at.unwrap_or(at)
            } else {
                attack.unwrap_or(at)
            };
            self.last_onset = Some(self.onset_at);
        }
        (self.high, onset)
    }
    pub(super) const fn published_pitch(&self) -> Option<f32> {
        self.published
    }
    fn amplitude_near(&self, at: u64) -> bool {
        self.flux
            .is_some_and(|stamp| stamp.abs_diff(at) <= self.amplitude_window)
            || self
                .amplitude_at
                .is_some_and(|stamp| stamp.abs_diff(at) <= self.amplitude_window)
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
        self.fresh_reading = false;
        self.amplitude_at = None;
        self.attack_at = None;
    }
}
