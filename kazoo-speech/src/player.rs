//! A real-time player for rendered phrases.
//!
//! A [`SpeechPlayer`] lives on the audio thread and plays one [`Phrase`] on
//! a gate. Its partner, the [`PhraseFeed`], lives on the control side: it
//! hands the player new phrases through a lock-free ring and takes the old
//! ones back through another, so a phrase's memory is only ever freed off
//! the audio thread.
//!
//! # Modes
//!
//! - **once**: a rising gate plays the phrase through once; the gate's fall
//!   is ignored. A new rising edge starts it again.
//! - **loop**: a rising gate starts the phrase and it goes round from the
//!   start offset forever. A new rising edge starts it again from the
//!   offset; turning the mode to once lets it finish the pass it is on.
//! - **gate**: the phrase plays while the gate is held and fades out when
//!   it falls.
//!
//! # Timing
//!
//! In **varispeed** timing, `rate` plays the phrase faster or slower and
//! its pitch follows, like a tape. In **stretch** timing, `rate` changes
//! only the pace: two Hann-windowed grains of 60 ms, half a grain apart,
//! overlap-add at the phrase's own pitch while the read point moves at
//! `rate`. The windows sum to one, so the stretch never clicks.
//!
//! # Timing accuracy
//!
//! The gate passed to [`SpeechPlayer::process`] is read per sample, so its
//! edges land on the exact sample. A host that drives the player by events
//! instead ([`SpeechPlayer::trigger`], [`SpeechPlayer::release`], or a
//! phrase loaded with `play`) makes them sample-accurate by splitting its
//! block at the event: process up to the event's frame, apply the event,
//! then process the rest. A phrase handed over through the feed is picked
//! up at the start of the next call to [`SpeechPlayer::process`].
//!
//! # Clicks
//!
//! Every read fades in over 2 ms from the start offset and out over the
//! last 2 ms of the phrase, so starts, ends and loop seams are smooth. When
//! a playing phrase is cut off (restarted, replaced or unloaded), its next
//! 3 ms are rendered with a fade into a pre-allocated tail that plays out
//! under whatever comes next.
//!
//! # Real-time contract
//!
//! [`SpeechPlayer::new`] allocates. [`SpeechPlayer::process`],
//! [`SpeechPlayer::set_param`], [`SpeechPlayer::trigger`],
//! [`SpeechPlayer::release`] and [`SpeechPlayer::reset`] never allocate,
//! lock, do I/O, panic or free a phrase. Dropping the player drops the
//! phrase it holds, so a host drops it off the audio thread.

use std::f64::consts::PI;
use std::fmt;
use std::sync::Arc;

use kazoo_fx::dsp::{Smoothed, hermite};
use kazoo_fx::{Curve, ParamSpec};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// A rendered phrase: mono samples at a known rate, shared cheaply.
#[derive(Clone, PartialEq)]
pub struct Phrase {
    samples: Arc<[f32]>,
    sample_rate: u32,
}

impl fmt::Debug for Phrase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Phrase")
            .field("frames", &self.samples.len())
            .field("sample_rate", &self.sample_rate)
            .finish()
    }
}

impl Phrase {
    /// A phrase of `samples` at `sample_rate` Hz. Non-finite samples become
    /// silence; a zero rate is taken as 48 kHz.
    #[must_use]
    pub fn new(mut samples: Vec<f32>, sample_rate: u32) -> Self {
        kazoo_fx::dsp::sanitise(&mut samples);
        Self {
            samples: samples.into(),
            sample_rate: if sample_rate == 0 {
                48_000
            } else {
                sample_rate
            },
        }
    }

    /// The samples.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// The samples, shared.
    #[must_use]
    pub fn shared(&self) -> Arc<[f32]> {
        Arc::clone(&self.samples)
    }

    /// The rate the samples were rendered at, in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How long it lasts at its own rate, in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(self.sample_rate)
    }
}

/// Parameter indices for [`SpeechPlayer::set_param`], in the order of
/// [`PARAMS`].
pub mod param {
    /// once, loop or gate.
    pub const MODE: usize = 0;
    /// Playback rate: speed in varispeed timing, pace in stretch timing.
    pub const RATE: usize = 1;
    /// varispeed or stretch.
    pub const TIMING: usize = 2;
    /// Where playback starts, as a fraction of the phrase.
    pub const START: usize = 3;
    /// Output level.
    pub const LEVEL: usize = 4;
    /// How many parameters there are.
    pub const COUNT: usize = 5;
}

static MODE_LABELS: [&str; 3] = ["once", "loop", "gate"];

static TIMING_LABELS: [&str; 2] = ["varispeed", "stretch"];

/// Every player parameter, numbered as [`param`] numbers them.
pub static PARAMS: [ParamSpec; param::COUNT] = [
    ParamSpec {
        name: "mode",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &MODE_LABELS,
        },
    },
    ParamSpec {
        name: "rate",
        min: 0.25,
        max: 4.0,
        default: 1.0,
        unit: "x",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "timing",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &TIMING_LABELS,
        },
    },
    ParamSpec {
        name: "start",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "level",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
];

/// How many phrases may be out with the player (queued, playing or on
/// their way back) at once.
const IN_FLIGHT: usize = 8;

/// Fade at every phrase edge, in seconds of the phrase.
const EDGE_SECONDS: f64 = 0.002;

/// Declick tail rendered when a playing phrase is cut off, in seconds.
const TAIL_SECONDS: f32 = 0.003;

/// Fade when a held gate falls, or a stretched phrase reaches its end.
const RELEASE_SECONDS: f32 = 0.005;

/// Grain length for stretch timing, in seconds of the phrase.
const GRAIN_SECONDS: f64 = 0.06;

/// Glide for the rate and level knobs.
const SMOOTH_SECONDS: f32 = 0.02;

/// What the feed hands the player: a phrase (empty to unload) and whether
/// to start it at once.
#[derive(Debug)]
struct Handover {
    phrase: Phrase,
    play: bool,
}

/// The control side of a [`SpeechPlayer`]: hands it phrases and frees the
/// ones it is done with. Never used on the audio thread.
pub struct PhraseFeed {
    outbox: HeapProd<Handover>,
    retired: HeapCons<Phrase>,
    outstanding: usize,
}

impl fmt::Debug for PhraseFeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhraseFeed")
            .field("outstanding", &self.outstanding)
            .finish_non_exhaustive()
    }
}

impl PhraseFeed {
    /// Hand the player `phrase`, starting it at once when `play` is set
    /// (otherwise it waits for the gate). Whatever it was playing is cut
    /// off with a short fade. Gives the phrase back if eight phrases are
    /// already out with the player: its audio is not running, or this is
    /// being called far faster than it could hear.
    pub fn load(&mut self, phrase: Phrase, play: bool) -> Result<(), Phrase> {
        self.collect();
        if self.outstanding >= IN_FLIGHT {
            return Err(phrase);
        }
        match self.outbox.try_push(Handover { phrase, play }) {
            Ok(()) => {
                self.outstanding += 1;
                Ok(())
            }
            Err(refused) => Err(refused.phrase),
        }
    }

    /// Take the player's phrase away, with a short fade if it is playing.
    /// Returns false, changing nothing, when eight phrases are already out
    /// with the player.
    pub fn unload(&mut self) -> bool {
        self.load(Phrase::new(Vec::new(), 0), false).is_ok()
    }

    /// Free every phrase the player has finished with; returns how many.
    /// [`Self::load`] and [`Self::unload`] do this first, but a host that
    /// loads rarely should call it now and then.
    pub fn collect(&mut self) -> usize {
        let mut freed = 0;
        while let Some(phrase) = self.retired.try_pop() {
            drop(phrase);
            freed += 1;
        }
        self.outstanding = self.outstanding.saturating_sub(freed);
        freed
    }

    /// Phrases handed over and not yet given back.
    #[must_use]
    pub const fn outstanding(&self) -> usize {
        self.outstanding
    }
}

/// One playhead.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    playing: bool,
    /// Varispeed: the read point. Stretch: where the grains start from.
    head: f64,
    /// The start offset this pass began at, in phrase samples.
    origin: f64,
    /// Stretch: where each grain began reading.
    grains: [f64; 2],
    /// Stretch: how far through grain 0 we are, 0 up to 1. Grain 1 is half
    /// a grain behind.
    phase: f64,
    /// Output gain, 0 to 1: falls to zero on release.
    envelope: f32,
    /// Change in envelope per sample: negative while releasing.
    envelope_step: f32,
}

/// What a voice needs to know about the phrase and the knobs this sample.
#[derive(Debug, Clone, Copy)]
struct Step {
    /// Phrase samples per output sample at rate 1.
    ratio: f64,
    rate: f64,
    stretch: bool,
    looping: bool,
    edge: f64,
    grain: f64,
    /// Fall in envelope per sample while releasing.
    fade: f32,
}

/// The phrase at a fractional position, with the edge fades, and silence
/// outside `origin..len`.
fn read(samples: &[f32], position: f64, origin: f64, edge: f64) -> f32 {
    let len = samples.len();
    if !(position >= origin && position < len as f64) || len == 0 {
        return 0.0;
    }
    let whole = position.floor();
    let frac = (position - whole) as f32;
    let index = whole as usize;
    let at = |offset: isize| {
        index
            .checked_add_signed(offset)
            .and_then(|i| samples.get(i))
            .copied()
            .unwrap_or(0.0)
    };
    let value = hermite(at(-1), at(0), at(1), at(2), frac);
    let fade_in = (position - origin) / edge;
    let fade_out = (len as f64 - 1.0 - position) / edge;
    value * fade_in.min(fade_out).clamp(0.0, 1.0) as f32
}

impl Voice {
    /// Start from `origin`. For stretch timing, grain 0 starts half-way
    /// through its window already reading from `origin` at full weight, and
    /// grain 1 starts silent, so the phrase begins at once.
    fn start(&mut self, origin: f64, grain: f64) {
        *self = Self {
            playing: true,
            head: origin,
            origin,
            grains: [0.5f64.mul_add(-grain, origin), origin],
            phase: 0.5,
            envelope: 1.0,
            envelope_step: 0.0,
        };
    }

    fn release(&mut self, step: f32) {
        if self.playing && self.envelope_step >= 0.0 {
            self.envelope_step = -step;
        }
    }

    /// The next sample, advancing the playhead.
    fn next(&mut self, samples: &[f32], step: &Step) -> f32 {
        if !self.playing {
            return 0.0;
        }
        let len = samples.len() as f64;
        let value = if step.stretch {
            self.stretched(samples, step)
        } else {
            let value = read(samples, self.head, self.origin, step.edge);
            self.head = step.ratio.mul_add(step.rate, self.head);
            value
        };
        let out = value * self.envelope;
        self.envelope = (self.envelope + self.envelope_step).clamp(0.0, 1.0);
        if self.envelope <= 0.0 {
            self.playing = false;
        }
        if self.head >= len - 1.0 {
            if step.looping {
                let overshoot = self.head - (len - 1.0);
                self.head = self.origin + overshoot.min(len - 1.0 - self.origin).max(0.0);
            } else if step.stretch {
                self.release(step.fade);
            } else {
                self.playing = false;
            }
        }
        out
    }

    /// Two windowed grains at the phrase's own pitch, overlap-added.
    fn stretched(&mut self, samples: &[f32], step: &Step) -> f32 {
        let mut sum = 0.0;
        for (grain, offset) in [(0usize, 0.0f64), (1, 0.5)] {
            let phase = (self.phase + offset).fract();
            let window = (PI * phase).sin().powi(2) as f32;
            let position = phase.mul_add(step.grain, self.grains[grain]);
            sum = read(samples, position, self.origin, step.edge).mul_add(window, sum);
        }
        // Each grain reads at the phrase's own speed; a grain whose window
        // has closed starts again from the read point.
        let before = self.phase;
        let mut after = before + step.ratio / step.grain;
        if before < 0.5 && after >= 0.5 {
            self.grains[1] = self.head;
        }
        if after >= 1.0 {
            after = (after - 1.0).min(0.5);
            self.grains[0] = self.head;
        }
        self.phase = after;
        self.head = step.ratio.mul_add(step.rate, self.head);
        sum
    }
}

/// Plays a rendered phrase on a gate: see the [module documentation](self).
pub struct SpeechPlayer {
    sample_rate: f32,
    phrase: Option<Phrase>,
    inbox: HeapCons<Handover>,
    retire: HeapProd<Phrase>,
    stranded: Option<Phrase>,
    values: [f32; param::COUNT],
    rate: Smoothed,
    level: Smoothed,
    gate: bool,
    voice: Voice,
    tail: Vec<f32>,
    tail_len: usize,
    tail_at: usize,
}

impl fmt::Debug for SpeechPlayer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpeechPlayer")
            .field("sample_rate", &self.sample_rate)
            .field("phrase", &self.phrase)
            .field("playing", &self.voice.playing)
            .field("values", &self.values)
            .finish_non_exhaustive()
    }
}

impl SpeechPlayer {
    /// A player for an engine running at `sample_rate` (a non-finite or
    /// non-positive rate is taken as 48 kHz), with its feed. Allocates.
    #[must_use]
    pub fn new(sample_rate: f32) -> (Self, PhraseFeed) {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let (outbox, inbox) = HeapRb::<Handover>::new(IN_FLIGHT).split();
        // Every phrase out with the player fits on the way back, so a
        // retirement never fails while the feed keeps count.
        let (retire, retired) = HeapRb::<Phrase>::new(IN_FLIGHT + 1).split();
        let tail_len = ((TAIL_SECONDS * sample_rate).round() as usize).max(1);
        let mut values = [0.0; param::COUNT];
        for (value, spec) in values.iter_mut().zip(&PARAMS) {
            *value = spec.default;
        }
        let mut rate = Smoothed::new(PARAMS[param::RATE].default);
        rate.set_time(SMOOTH_SECONDS, sample_rate);
        let mut level = Smoothed::new(PARAMS[param::LEVEL].default);
        level.set_time(SMOOTH_SECONDS, sample_rate);
        let player = Self {
            sample_rate,
            phrase: None,
            inbox,
            retire,
            stranded: None,
            values,
            rate,
            level,
            gate: false,
            voice: Voice::default(),
            tail: vec![0.0; tail_len],
            tail_len,
            tail_at: tail_len,
        };
        let feed = PhraseFeed {
            outbox,
            retired,
            outstanding: 0,
        };
        (player, feed)
    }

    /// Set parameter `index` (see [`param`]) to `value`, held within its
    /// range. A NaN or an index out of range is ignored. Real-time safe.
    pub fn set_param(&mut self, index: usize, value: f32) {
        if value.is_nan() {
            return;
        }
        let Some(spec) = PARAMS.get(index) else {
            return;
        };
        let value = spec.clamp(value);
        self.values[index] = value;
        match index {
            param::RATE => self.rate.set(value),
            param::LEVEL => self.level.set(value),
            _ => {}
        }
    }

    /// The value parameter `index` was last set to (its default if never
    /// set), or `None` for an index out of range.
    #[must_use]
    pub fn param(&self, index: usize) -> Option<f32> {
        self.values.get(index).copied()
    }

    /// Whether a phrase is sounding.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        self.voice.playing
    }

    /// The phrase loaded, if any.
    #[must_use]
    pub fn phrase(&self) -> Option<&Phrase> {
        self.phrase
            .as_ref()
            .filter(|phrase| !phrase.samples.is_empty())
    }

    /// How far through the phrase the playhead is, 0 to 1.
    #[must_use]
    pub fn progress(&self) -> f32 {
        let len = self
            .phrase
            .as_ref()
            .map_or(0, |phrase| phrase.samples.len());
        if len == 0 || !self.voice.playing {
            return 0.0;
        }
        (self.voice.head / len as f64).clamp(0.0, 1.0) as f32
    }

    /// Start the phrase from the start offset now, as a rising gate would.
    /// Real-time safe.
    pub fn trigger(&mut self) {
        self.cut();
        let origin = self.origin();
        if let Some(phrase) = self
            .phrase
            .as_ref()
            .filter(|phrase| !phrase.samples.is_empty())
        {
            let grain = self.step(phrase, self.rate.value()).grain;
            self.voice.start(origin, grain);
        }
    }

    /// Let go, as a falling gate would: in gate mode the phrase fades out;
    /// in the other modes nothing happens. Real-time safe.
    pub fn release(&mut self) {
        if self.mode() == Mode::Gate {
            let step = self.release_step();
            self.voice.release(step);
        }
    }

    /// Stop at once and forget the gate, keeping the phrase and the
    /// parameters. Real-time safe.
    pub fn reset(&mut self) {
        self.voice = Voice::default();
        self.gate = false;
        self.tail_at = self.tail_len;
        self.rate.snap(self.values[param::RATE]);
        self.level.snap(self.values[param::LEVEL]);
    }

    /// Render one block into `output`. `gate` is read per sample (high
    /// above 0.5); if it is shorter than `output`, its last value (or the
    /// gate as it last stood, if it is empty) holds for the rest. Real-time
    /// safe.
    pub fn process(&mut self, gate: &[f32], output: &mut [f32]) {
        self.take_handovers();
        let mut last = self.gate;
        for (frame, out) in output.iter_mut().enumerate() {
            let high = gate.get(frame).map_or(last, |&value| value > 0.5);
            last = high;
            if high && !self.gate {
                self.trigger();
            } else if !high && self.gate {
                self.release();
            }
            self.gate = high;
            *out = self.next_sample();
        }
    }

    fn next_sample(&mut self) -> f32 {
        let rate = self.rate.step();
        let level = self.level.step();
        let tail = if self.tail_at < self.tail_len {
            let value = self.tail[self.tail_at];
            self.tail_at += 1;
            value
        } else {
            0.0
        };
        let voice = match &self.phrase {
            Some(phrase) if self.voice.playing => {
                let step = self.step(phrase, rate);
                self.voice.next(&phrase.samples, &step)
            }
            _ => 0.0,
        };
        let out = (voice + tail) * level;
        if out.is_finite() { out } else { 0.0 }
    }

    fn step(&self, phrase: &Phrase, rate: f32) -> Step {
        let ratio = f64::from(phrase.sample_rate) / f64::from(self.sample_rate);
        let rate = f64::from(rate).clamp(
            f64::from(PARAMS[param::RATE].min),
            f64::from(PARAMS[param::RATE].max),
        );
        Step {
            ratio,
            rate,
            stretch: self.values[param::TIMING] >= 0.5,
            looping: self.mode() == Mode::Loop,
            edge: (EDGE_SECONDS * f64::from(phrase.sample_rate)).max(1.0),
            grain: (GRAIN_SECONDS * f64::from(phrase.sample_rate)).max(4.0),
            fade: self.release_step(),
        }
    }

    fn release_step(&self) -> f32 {
        1.0 / (RELEASE_SECONDS * self.sample_rate).max(1.0)
    }

    const fn mode(&self) -> Mode {
        match self.values[param::MODE] as u8 {
            1 => Mode::Loop,
            2 => Mode::Gate,
            _ => Mode::Once,
        }
    }

    fn origin(&self) -> f64 {
        let len = self
            .phrase
            .as_ref()
            .map_or(0, |phrase| phrase.samples.len());
        let start = f64::from(self.values[param::START]) * len as f64;
        start.clamp(0.0, (len as f64 - 2.0).max(0.0)).floor()
    }

    /// If a phrase is sounding, render its next few milliseconds with a
    /// fade into the tail and stop it.
    fn cut(&mut self) {
        let Some(phrase) = &self.phrase else {
            self.voice.playing = false;
            return;
        };
        if !self.voice.playing {
            return;
        }
        let mut step = self.step(phrase, self.rate.value());
        step.looping = true;
        // Fold whatever is left of an older tail into the new one. The
        // older value at `tail_at + index` is read before `index` is
        // written, and later reads are never below the last write.
        let remaining = self.tail_len.saturating_sub(self.tail_at);
        let mut voice = self.voice;
        let len = self.tail_len;
        for index in 0..len {
            let fade = 1.0 - index as f32 / len as f32;
            let carried = if index < remaining {
                self.tail[self.tail_at + index]
            } else {
                0.0
            };
            self.tail[index] = voice.next(&phrase.samples, &step).mul_add(fade, carried);
        }
        self.tail_at = 0;
        self.voice.playing = false;
    }

    /// Pick up whatever the feed has sent, retiring replaced phrases.
    fn take_handovers(&mut self) {
        if let Some(phrase) = self.stranded.take() {
            if let Err(phrase) = self.retire.try_push(phrase) {
                self.stranded = Some(phrase);
                return;
            }
        }
        while let Some(Handover { phrase, play }) = self.inbox.try_pop() {
            self.cut();
            // An empty phrase (an unload) is held like any other, so each
            // handover retires at most one phrase.
            if let Some(old) = self.phrase.replace(phrase) {
                // The ring always has room while the feed keeps count;
                // should it ever not, the phrase waits here rather than
                // being freed on the audio thread.
                if let Err(old) = self.retire.try_push(old) {
                    self.stranded = Some(old);
                }
            }
            if play {
                self.trigger();
            }
            if self.stranded.is_some() {
                return;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Once,
    Loop,
    Gate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn sine_phrase(hz: f32, seconds: f32) -> Phrase {
        let len = (seconds * RATE) as usize;
        Phrase::new(
            (0..len)
                .map(|n| 0.5 * (TAU * hz * n as f32 / RATE).sin())
                .collect(),
            RATE as u32,
        )
    }

    fn loaded(phrase: Phrase) -> (SpeechPlayer, PhraseFeed) {
        let (mut player, mut feed) = SpeechPlayer::new(RATE);
        assert!(feed.load(phrase, false).is_ok());
        player.process(&[], &mut [0.0; 1]);
        (player, feed)
    }

    fn render(player: &mut SpeechPlayer, gate: f32, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        let gate = vec![gate; frames];
        for (g, o) in gate.chunks(128).zip(out.chunks_mut(128)) {
            player.process(g, o);
        }
        out
    }

    fn peak(block: &[f32]) -> f32 {
        block.iter().fold(0.0f32, |p, x| p.max(x.abs()))
    }

    fn crossings(block: &[f32]) -> usize {
        block
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count()
    }

    fn biggest_step(block: &[f32]) -> f32 {
        block
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn once_plays_through_and_stops_whatever_the_gate_does() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 0.1));
        assert!(peak(&render(&mut player, 0.0, 480)) < f32::EPSILON);
        let mut gate = vec![1.0; 480];
        gate[240..].fill(0.0);
        let mut out = vec![0.0; 480];
        player.process(&gate, &mut out);
        assert!(player.is_playing());
        let rest = render(&mut player, 0.0, 9_600);
        assert!(peak(&rest[..3_000]) > 0.3);
        assert!(!player.is_playing());
        assert!(peak(&rest[4_800..]) < f32::EPSILON);
    }

    #[test]
    fn loop_goes_round_until_the_mode_changes() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 0.05));
        player.set_param(param::MODE, 1.0);
        let out = render(&mut player, 1.0, 24_000);
        assert!(player.is_playing());
        assert!(peak(&out[21_000..]) > 0.3);
        player.set_param(param::MODE, 0.0);
        render(&mut player, 1.0, 4_800);
        assert!(!player.is_playing());
    }

    #[test]
    fn gate_mode_fades_out_when_the_gate_falls() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 1.0));
        player.set_param(param::MODE, 2.0);
        let held = render(&mut player, 1.0, 4_800);
        assert!(peak(&held[1_000..]) > 0.3);
        let released = render(&mut player, 0.0, 4_800);
        assert!(!player.is_playing());
        let fade = (RELEASE_SECONDS * RATE) as usize + 2;
        assert!(peak(&released[fade..]) < f32::EPSILON);
        assert!(biggest_step(&released) < 0.1);
    }

    #[test]
    fn gate_edges_land_on_their_sample() {
        let (mut player, _feed) = loaded(Phrase::new(vec![1.0; 48_000], RATE as u32));
        let mut gate = [0.0; 256];
        gate[100..].fill(1.0);
        let mut out = [0.0; 256];
        player.process(&gate, &mut out);
        assert!(out[..=100].iter().all(|x| x.abs() < f32::EPSILON));
        assert!(out[101] > 0.0);
        // Faded in within the edge time.
        assert!((out[100 + (EDGE_SECONDS * f64::from(RATE)) as usize + 2] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn events_start_and_release_between_split_blocks() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 1.0));
        player.set_param(param::MODE, 2.0);
        let mut out = [0.0; 64];
        player.process(&[], &mut out[..10]);
        player.trigger();
        player.process(&[], &mut out[10..]);
        assert!(out[..10].iter().all(|x| x.abs() < f32::EPSILON));
        assert!(player.is_playing());
        player.release();
        render(&mut player, 0.0, 1_000);
        assert!(!player.is_playing());
    }

    #[test]
    fn varispeed_changes_length_and_pitch() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 0.5));
        player.set_param(param::RATE, 2.0);
        player.reset();
        let out = render(&mut player, 1.0, 24_000);
        assert!(!player.is_playing());
        // An octave up over a quarter of a second.
        let heard = crossings(&out[..11_000]);
        let expected = 2 * 880 * 11_000 / 48_000;
        assert!(heard.abs_diff(expected) < 12, "{heard} vs {expected}");
    }

    #[test]
    fn stretch_changes_length_but_not_pitch() {
        let (mut player, _feed) = loaded(sine_phrase(440.0, 0.25));
        player.set_param(param::TIMING, 1.0);
        player.set_param(param::RATE, 0.5);
        player.reset();
        let out = render(&mut player, 1.0, 20_000);
        // Twice as long: still sounding past the phrase's own length.
        assert!(peak(&out[14_000..20_000]) > 0.2);
        let heard = crossings(&out[2_000..20_000]);
        let expected = 2 * 440 * 18_000 / 48_000;
        assert!(
            heard.abs_diff(expected) < expected / 20,
            "{heard} vs {expected}"
        );
        let rest = render(&mut player, 1.0, 24_000);
        assert!(!player.is_playing());
        assert!(biggest_step(&out) < 0.2 && biggest_step(&rest) < 0.2);
    }

    #[test]
    fn start_offset_skips_ahead() {
        // First half silent, second half loud.
        let mut samples = vec![0.0; 9_600];
        samples[4_800..].fill(0.5);
        let (mut player, _feed) = loaded(Phrase::new(samples, RATE as u32));
        player.set_param(param::START, 0.5);
        let out = render(&mut player, 1.0, 2_000);
        assert!(peak(&out[200..2_000]) > 0.45);
    }

    #[test]
    fn retriggering_mid_phrase_does_not_click() {
        let (mut player, _feed) = loaded(sine_phrase(220.0, 1.0));
        player.set_param(param::START, 0.3);
        let mut out = render(&mut player, 1.0, 1_000);
        out.extend(render(&mut player, 0.0, 17));
        out.extend(render(&mut player, 1.0, 2_000));
        // A 220 Hz sine at 0.5 moves at most about 0.015 a sample.
        assert!(biggest_step(&out) < 0.05, "{}", biggest_step(&out));
    }

    #[test]
    fn phrases_come_back_to_the_feed() {
        let (mut player, mut feed) = SpeechPlayer::new(RATE);
        let first = sine_phrase(440.0, 0.1);
        let watcher = first.shared();
        assert!(feed.load(first, true).is_ok());
        render(&mut player, 0.0, 256);
        assert!(player.is_playing());
        assert!(feed.load(sine_phrase(330.0, 0.1), false).is_ok());
        render(&mut player, 0.0, 256);
        // Cut off by the new phrase, which waits for the gate.
        assert!(!player.is_playing());
        assert_eq!(Arc::strong_count(&watcher), 2);
        assert_eq!(feed.collect(), 1);
        assert_eq!(Arc::strong_count(&watcher), 1);
        assert_eq!(feed.outstanding(), 1);
        assert!(feed.unload());
        render(&mut player, 0.0, 16);
        assert!(player.phrase().is_none());
        assert_eq!(feed.collect(), 1);
    }

    #[test]
    fn a_stalled_player_refuses_more_than_it_can_give_back() {
        let (player, mut feed) = SpeechPlayer::new(RATE);
        for _ in 0..IN_FLIGHT {
            assert!(feed.load(sine_phrase(440.0, 0.01), false).is_ok());
        }
        assert!(feed.load(sine_phrase(440.0, 0.01), false).is_err());
        assert!(!feed.unload());
        drop(player);
    }

    #[test]
    fn poison_is_refused() {
        let (mut player, _feed) = loaded(Phrase::new(
            vec![f32::NAN, f32::INFINITY, 0.5, f32::NEG_INFINITY],
            RATE as u32,
        ));
        for (index, spec) in PARAMS.iter().enumerate() {
            player.set_param(index, f32::NAN);
            assert_eq!(player.param(index), Some(spec.default));
        }
        let out = render(&mut player, f32::NAN, 64);
        assert!(!player.is_playing());
        assert!(out.iter().all(|x| x.abs() < f32::EPSILON));
        let out = render(&mut player, 1.0, 64);
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(
            player
                .phrase()
                .is_some_and(|p| p.samples().iter().all(|x| x.is_finite()))
        );
    }

    #[test]
    fn nothing_loaded_is_silence() {
        let (mut player, _feed) = SpeechPlayer::new(f32::NAN);
        let out = render(&mut player, 1.0, 256);
        assert!(out.iter().all(|x| x.abs() < f32::EPSILON));
        assert!(!player.is_playing());
        assert!(player.progress().abs() < f32::EPSILON);
    }

    #[test]
    fn phrases_at_another_rate_play_at_their_own_speed() {
        let phrase = Phrase::new(vec![0.25; 24_000], 24_000);
        assert!((phrase.seconds() - 1.0).abs() < 1e-9);
        let (mut player, _feed) = loaded(phrase);
        render(&mut player, 1.0, 40_000);
        assert!(player.is_playing());
        render(&mut player, 1.0, 10_000);
        assert!(!player.is_playing());
    }
}
