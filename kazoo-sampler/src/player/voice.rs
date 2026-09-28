//! One playing voice: where it is in the sample, how it moves, its envelope
//! and, in granular mode, its grains.

use kazoo_fx::dsp::Noise;

use super::envelope::Envelope;
use super::params::{GrainSettings, Mode, Settings};
use crate::SampleData;
use crate::sample::MAX_OCTAVES_UP;

/// Grains one voice can have sounding at once.
pub(crate) const MAX_GRAINS: usize = 32;

/// The lowest a voice can be pitched, in octaves below the original.
const LOWEST_OCTAVE: f32 = -8.0;

/// The shortest loop or region, in frames.
const MIN_SPAN: f64 = 16.0;

/// Which of the player's samples a voice reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The loaded sample.
    Current,
    /// The sample being swapped out, while its voices fade.
    Outgoing,
}

/// The playable stretch of a sample for one direction, in playback
/// coordinates: frames from the start of the sample going forwards, or from
/// its end going backwards.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    pub(crate) frames: f64,
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) loop_start: f64,
    pub(crate) loop_end: f64,
}

impl Region {
    /// The knobs' region in a sample of `frames` frames.
    pub(crate) fn new(settings: &Settings, frames: usize, reverse: bool) -> Self {
        let total = frames as f64;
        let span = MIN_SPAN.min(total);
        let (start, end) = ordered(
            settings.start * total,
            settings.end * total,
            span,
            0.0,
            total,
        );
        let (loop_start, loop_end) = ordered(
            settings.loop_start * total,
            settings.loop_end * total,
            span.min(end - start),
            start,
            end,
        );
        if reverse {
            Self {
                frames: total,
                start: total - end,
                end: total - start,
                loop_start: total - loop_end,
                loop_end: total - loop_start,
            }
        } else {
            Self {
                frames: total,
                start,
                end,
                loop_start,
                loop_end,
            }
        }
    }
}

/// `a` and `b` in order, at least `span` apart, inside `low` to `high`.
fn ordered(a: f64, b: f64, span: f64, low: f64, high: f64) -> (f64, f64) {
    let (mut first, mut last) = (a.min(b).clamp(low, high), a.max(b).clamp(low, high));
    if last - first < span {
        last = (first + span).min(high);
        first = (last - span).max(low);
    }
    (first, last)
}

/// What a voice needs to know about its sample and the knobs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Context<'a> {
    pub(crate) data: &'a SampleData,
    pub(crate) settings: &'a Settings,
    /// Regions for playing forwards and backwards.
    pub(crate) regions: [Region; 2],
    /// Sample frames per output frame at the original pitch.
    pub(crate) rate_ratio: f64,
    /// Fade length at a region's end, in output frames.
    pub(crate) edge: f64,
    /// A Hann window, 0 to 1 over its length.
    pub(crate) hann: &'a [f32],
}

/// One grain.
#[derive(Debug, Clone, Copy)]
struct Grain {
    active: bool,
    /// Where it reads, in sample frames.
    position: f64,
    /// Frames it moves per output frame; negative plays backwards.
    step: f64,
    /// Output frames since it started.
    age: f64,
    /// Output frames it lasts.
    length: f64,
}

const SILENT_GRAIN: Grain = Grain {
    active: false,
    position: 0.0,
    step: 1.0,
    age: 0.0,
    length: 1.0,
};

/// How to start a voice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Start {
    pub(crate) age: u64,
    pub(crate) key: Option<u8>,
    pub(crate) follows_cv: bool,
    /// Pitch from the note or the V/oct input, in octaves.
    pub(crate) note: f32,
    pub(crate) gain: f32,
    pub(crate) mode: Mode,
    pub(crate) reverse: bool,
    /// Slice bounds in playback coordinates (slice mode only).
    pub(crate) slice: (f64, f64),
    /// Where to begin, in playback coordinates.
    pub(crate) position: f64,
    /// Where the grain scan begins, relative to the position knob.
    pub(crate) scan: f64,
}

/// A voice.
#[derive(Debug, Clone)]
pub(crate) struct Voice {
    pub(crate) active: bool,
    pub(crate) age: u64,
    pub(crate) source: Source,
    pub(crate) held: bool,
    pub(crate) key: Option<u8>,
    pub(crate) follows_cv: bool,
    pub(crate) note: f32,
    mode: Mode,
    reverse: bool,
    gain: f32,
    env: Envelope,
    fade: f32,
    fade_step: f32,
    position: f64,
    forward: bool,
    wrapped: bool,
    slice: (f64, f64),
    grains: [Grain; MAX_GRAINS],
    grain_timer: f64,
    scan: f64,
    noise: Noise,
}

impl Voice {
    pub(crate) const fn new(seed: u32) -> Self {
        Self {
            active: false,
            age: 0,
            source: Source::Current,
            held: false,
            key: None,
            follows_cv: false,
            note: 0.0,
            mode: Mode::OneShot,
            reverse: false,
            gain: 1.0,
            env: Envelope::new(),
            fade: 1.0,
            fade_step: 0.0,
            position: 0.0,
            forward: true,
            wrapped: false,
            slice: (0.0, 0.0),
            grains: [SILENT_GRAIN; MAX_GRAINS],
            grain_timer: 0.0,
            scan: 0.0,
            noise: Noise::new(seed),
        }
    }

    /// Begin playing, from silence.
    pub(crate) fn start(&mut self, start: &Start) {
        self.active = true;
        self.age = start.age;
        self.source = Source::Current;
        self.held = true;
        self.key = start.key;
        self.follows_cv = start.follows_cv;
        self.note = start.note;
        self.mode = start.mode;
        self.reverse = start.reverse;
        self.gain = start.gain;
        self.env.reset();
        self.env.gate_on();
        self.fade = 1.0;
        self.fade_step = 0.0;
        self.position = start.position;
        self.forward = true;
        self.wrapped = false;
        self.slice = start.slice;
        self.grains = [SILENT_GRAIN; MAX_GRAINS];
        self.grain_timer = 0.0;
        self.scan = start.scan;
    }

    /// The gate or key has let go.
    pub(crate) fn release(&mut self) {
        self.held = false;
        self.follows_cv = false;
        if self.mode.is_held() {
            self.env.gate_off();
        }
    }

    /// Fade out over `frames` output frames, then stop: for a stolen voice
    /// or a sample being swapped out.
    pub(crate) fn fade_out(&mut self, frames: f32) {
        self.held = false;
        self.follows_cv = false;
        self.fade_step = self.fade_step.max(1.0 / frames.max(1.0));
    }

    /// Whether the voice is fading out to make room.
    pub(crate) fn is_fading(&self) -> bool {
        self.fade_step > 0.0
    }

    /// How loud the fade is now, 0 to 1.
    pub(crate) const fn fade_level(&self) -> f32 {
        self.fade
    }

    /// Stop at once.
    pub(crate) fn stop(&mut self) {
        self.active = false;
        self.held = false;
        self.follows_cv = false;
        self.env.reset();
    }

    /// The playhead, as a fraction of the sample from its start.
    pub(crate) fn playhead(&self, frames: usize) -> f32 {
        let total = frames.max(1) as f64;
        let physical = if self.reverse {
            total - 1.0 - self.position
        } else {
            self.position
        };
        (physical / total).clamp(0.0, 1.0) as f32
    }

    /// The next stereo frame.
    pub(crate) fn tick(&mut self, context: &Context<'_>) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let octaves =
            (context.settings.transpose + self.note).clamp(LOWEST_OCTAVE, MAX_OCTAVES_UP as f32);
        let speed = f64::from(octaves.exp2()) * context.rate_ratio;
        let played = if self.mode == Mode::Granular {
            Some(self.grains(context, speed))
        } else {
            self.play(context, speed)
        };
        let level = self.env.next(&context.settings.env);
        if self.fade_step > 0.0 {
            self.fade -= self.fade_step;
        }
        let Some((left, right)) = played else {
            self.stop();
            return (0.0, 0.0);
        };
        if self.env.is_idle() || self.fade <= 0.0 {
            self.stop();
            return (0.0, 0.0);
        }
        let gain = level * self.gain * self.fade;
        (left * gain, right * gain)
    }

    /// The sample at playback position `position`.
    fn read(&self, context: &Context<'_>, position: f64, speed: f64) -> (f32, f32) {
        let physical = if self.reverse {
            context.regions[1].frames - 1.0 - position
        } else {
            position
        };
        context.data.read(physical, speed)
    }

    /// One frame of the sample-playing modes; `None` once past the end.
    fn play(&mut self, context: &Context<'_>, speed: f64) -> Option<(f32, f32)> {
        let region = context.regions[usize::from(self.reverse)];
        match self.mode {
            // Granular voices never come here; they read through their
            // grains.
            Mode::OneShot | Mode::Gate | Mode::Slice | Mode::Granular => {
                let (_, end) = if self.mode == Mode::Slice {
                    self.slice
                } else {
                    (region.start, region.end)
                };
                if self.position >= end {
                    return None;
                }
                // Fade into a region's end so a cut never clicks.
                let edge = (((end - self.position) / speed) / context.edge).min(1.0) as f32;
                let (left, right) = self.read(context, self.position, speed);
                self.position += speed;
                Some((left * edge, right * edge))
            }
            Mode::Loop => {
                let length = region.loop_end - region.loop_start;
                if self.position >= region.loop_end {
                    self.position =
                        region.loop_start + (self.position - region.loop_end).rem_euclid(length);
                    self.wrapped = true;
                }
                let out = self.loop_read(context, &region, speed, length);
                self.position += speed;
                Some(out)
            }
            Mode::PingPong => {
                let out = self.read(context, self.position, speed);
                self.bounce(&region, speed);
                Some(out)
            }
        }
    }

    /// A loop frame, crossfaded across the loop point. The fade uses the
    /// audio before the loop start when there is enough of it (the end of
    /// the loop fades into what leads into its start), otherwise the audio
    /// after the loop end (the start of the loop fades in from what follows
    /// its end). Either way the wrap itself is seamless.
    fn loop_read(
        &self,
        context: &Context<'_>,
        region: &Region,
        speed: f64,
        length: f64,
    ) -> (f32, f32) {
        let wanted =
            (context.settings.crossfade * f64::from(context.data.rate())).min(length / 2.0);
        let before = wanted.min(region.loop_start);
        let after = wanted.min(region.frames - region.loop_end);
        let here = self.read(context, self.position, speed);
        if before >= after && before >= 1.0 {
            let from = region.loop_end - before;
            if self.position > from {
                let t = ((self.position - from) / before) as f32;
                let lead = self.read(context, self.position - length, speed);
                return mix(here, lead, t);
            }
        } else if after >= 1.0 && self.wrapped && self.position < region.loop_start + after {
            let t = ((self.position - region.loop_start) / after) as f32;
            let tail = self.read(context, self.position + length, speed);
            return mix(tail, here, t);
        }
        here
    }

    /// Move back and forth between the loop points.
    fn bounce(&mut self, region: &Region, speed: f64) {
        let (low, high) = (region.loop_start, region.loop_end);
        if self.forward {
            self.position += speed;
            if self.position >= high {
                self.position = high - (self.position - high);
                self.forward = false;
            }
        } else {
            self.position -= speed;
            if self.position <= low {
                self.position = low + (low - self.position);
                self.forward = true;
            }
        }
        if !self.forward || self.position >= low {
            self.position = self.position.clamp(low, high);
        }
    }

    /// One frame of the grain cloud.
    fn grains(&mut self, context: &Context<'_>, speed: f64) -> (f32, f32) {
        let settings = &context.settings.grain;
        let region = context.regions[0];
        let span = (region.end - region.start).max(1.0);
        if !settings.freeze {
            self.scan = (self.scan + context.rate_ratio).rem_euclid(span);
        }
        let anchor = settings.position * region.frames - region.start;
        let centre = region.start + (anchor + self.scan).rem_euclid(span);
        self.grain_timer -= 1.0;
        if self.grain_timer <= 0.0 {
            self.grain_timer = (self.grain_timer + settings.interval).max(1.0);
            self.spawn(settings, &region, centre, speed, context.rate_ratio);
        }
        let (mut left, mut right) = (0.0f32, 0.0f32);
        let last = (context.hann.len() - 1) as f64;
        for grain in &mut self.grains {
            if !grain.active {
                continue;
            }
            let at = grain.age / grain.length * last;
            let index = at as usize;
            let frac = (at - index as f64) as f32;
            let a = context.hann[index.min(context.hann.len() - 1)];
            let b = context.hann[(index + 1).min(context.hann.len() - 1)];
            let window = (b - a).mul_add(frac, a);
            let (l, r) = context.data.read(grain.position, grain.step);
            left = l.mul_add(window, left);
            right = r.mul_add(window, right);
            grain.position += grain.step;
            grain.age += 1.0;
            if grain.age >= grain.length {
                grain.active = false;
            }
        }
        (left * settings.gain, right * settings.gain)
    }

    /// Start a grain near `centre`, if one is free. `rate_ratio` turns the
    /// spray (in output frames) into sample frames.
    fn spawn(
        &mut self,
        settings: &GrainSettings,
        region: &Region,
        centre: f64,
        speed: f64,
        rate_ratio: f64,
    ) {
        let Some(slot) = self.grains.iter().position(|grain| !grain.active) else {
            return;
        };
        let offset = f64::from(self.noise.sample()) * settings.spray * rate_ratio;
        let position = (centre + offset).clamp(region.start, (region.end - 1.0).max(region.start));
        let detune = (self.noise.sample() * settings.spread / 12.0).exp2();
        let step = speed * f64::from(detune);
        self.grains[slot] = Grain {
            active: true,
            position,
            step: if self.reverse { -step } else { step },
            age: 0.0,
            length: settings.size,
        };
    }
}

/// `a` faded into `b` by `t`.
fn mix(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    ((b.0 - a.0).mul_add(t, a.0), (b.1 - a.1).mul_add(t, a.1))
}
