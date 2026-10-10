//! Measuring a signal on the audio thread: what the `scope` and `readout`
//! modules see, and the fixed-size [`Capture`] they hand the engine.
//!
//! A [`Meter`] watches one signal sample by sample and closes a reading
//! every [`WINDOW_SECONDS`]: its lowest, highest, mean, RMS and peak over
//! the window; its frequency, from rising crossings of its own middle with
//! hysteresis, timed between samples and averaged over the latest periods;
//! its rate and duty as a gate; and its lowest and highest since it was
//! last cleared. A sample that is not a number is counted as a fault and
//! measured as nothing, so a fault upstream shows as a fault, never as a
//! number. Nothing here allocates, locks or panics.
//!
//! The periods are read as a whole, not one by one (see [`History`]): when
//! they alternate long and short, each pair is one cycle (a wave with a
//! strong second harmonic crosses its middle twice a cycle; a swung clock
//! rises twice a pair of beats), and a gate whose gaps are whole numbers of
//! its shortest (a sequence with rests) runs at the rate of that shortest.
//! As soon as the cycle under way runs past the longest period kept, the
//! reading says it has paused (its last rate, not steady); what has been
//! learnt is let go only when nothing has crossed for
//! [`LONGEST_PERIOD_SECONDS`], so a rest, a long swung beat or a slowing
//! wave never clears it. A wave whose two newest periods agree with each other and are
//! far from the one before has changed its rate: the older periods are
//! let go, so a step from 2 Hz to 0.2 Hz reads true within two new
//! periods (a gate keeps them all, to know its rests).
//!
//! A [`Capture`] is plain data of a fixed size: the engine copies it into
//! a ring the control side drains (see [`crate::engine::ProbeFrame`]).

use super::{Edge, finite};

/// Points across a scope's screen, at most.
pub const TRACE_POINTS: usize = 96;

/// How long a [`Meter`]'s window is: its reading is this fresh.
pub const WINDOW_SECONDS: f32 = 0.1;

/// The shortest time between two captures a scope hands on: it hands on
/// at most 25 a second, however fast its sweeps.
pub const PUBLISH_GAP_SECONDS: f32 = 0.04;

/// The longest period measured: anything slower does not repeat, as far
/// as a meter can tell.
pub const LONGEST_PERIOD_SECONDS: f32 = 20.0;

/// Periods kept to average: enough that a high pitch, whose crossings are
/// timed between samples with a little error each, averages it away.
const HISTORY: usize = 32;

/// Periods are averaged over about this long (at least one is used).
const AVERAGE_SECONDS: f64 = 0.25;

/// Periods that differ by no more than this share of their mean are
/// steady.
const STEADY_SPREAD: f64 = 0.02;

/// Two periods within this share of each other agree, for telling a
/// changed rate (see [`History::follow_change`]).
const CHANGE_AGREE: f64 = 0.1;

/// A wave whose newest periods are this many times longer (or shorter)
/// than the one before has changed its rate.
const CHANGE_RATIO: f64 = 1.5;

/// How far a gap may be from a whole number of the shortest period and
/// still count as that many periods with rests between.
const WHOLE_SPREAD: f64 = 0.03;

/// The least swing a frequency is measured on: −80 dB from top to bottom.
const LEAST_SWING: f32 = 1.0e-4;

/// The hysteresis either side of a signal's middle, as a share of half its
/// swing: a crossing counts once the signal has gone this far below the
/// middle and then this far above it.
const HYSTERESIS: f32 = 0.15;

/// A sample within this share of the swing of the window before's lowest
/// or highest is held there: a square wave spends nearly all its time so,
/// a sine under half, a triangle or a saw a fifth.
const HELD_REACH: f32 = 0.1;

/// What a [`Meter`] measured over one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// Whether a cable was in.
    pub connected: bool,
    /// Samples measured: the window's numbers.
    pub frames: u32,
    /// Samples in the window that were not numbers (a fault upstream).
    pub faults: u32,
    /// Samples that were not numbers since it was last cleared.
    pub faults_since: u32,
    /// The latest number.
    pub last: f32,
    /// Lowest over the window.
    pub low: f32,
    /// Highest over the window.
    pub high: f32,
    /// Mean over the window: the DC offset.
    pub mean: f32,
    /// Root mean square over the window.
    pub rms: f32,
    /// Largest magnitude over the window.
    pub peak: f32,
    /// How often it repeats, in Hz; 0 when it does not.
    pub hz: f32,
    /// Whether its latest periods agree with each other (never while
    /// paused).
    pub steady: bool,
    /// Whether the cycle under way has run past the longest period kept:
    /// it has stopped, or slowed (`hz` is then its last rate).
    pub paused: bool,
    /// Of the window's steps that moved, the share that rose.
    pub rising: f32,
    /// The share of the window's samples held near the lowest or highest
    /// of the window before (see [`HELD_REACH`]).
    pub held: f32,
    /// As a gate.
    pub gate: GateReading,
    /// Lowest since it was last cleared; above `max` when nothing has been
    /// measured since.
    pub min: f32,
    /// Highest since it was last cleared.
    pub max: f32,
}

impl Reading {
    /// Nothing measured yet.
    pub const NONE: Self = Self {
        connected: false,
        frames: 0,
        faults: 0,
        faults_since: 0,
        last: 0.0,
        low: 0.0,
        high: 0.0,
        mean: 0.0,
        rms: 0.0,
        peak: 0.0,
        hz: 0.0,
        steady: false,
        paused: false,
        rising: 0.0,
        held: 0.0,
        gate: GateReading::NONE,
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
    };
}

/// What a [`Meter`] measured of its signal as a gate (high above 0.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateReading {
    /// How often it rises, in Hz; 0 when it does not.
    pub hz: f32,
    /// The share of a pulse's time its latest pulse was high (its length
    /// over the time between pulses, rests left out); negative when there
    /// is none.
    pub duty: f32,
    /// Whether it is high now.
    pub high: bool,
    /// Whether it has gone without a rise for longer than the longest gap
    /// kept (`hz` is then its last rate).
    pub paused: bool,
}

impl GateReading {
    /// No gate measured.
    pub const NONE: Self = Self {
        hz: 0.0,
        duty: -1.0,
        high: false,
        paused: false,
    };
}

/// A scope's trace: for each point across the screen, the lowest and
/// highest of each input over that point's frames; or, in XY mode, input
/// a's and b's sample at each point (`low` and `high` alike).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trace {
    /// Points across the screen.
    pub capacity: u16,
    /// Points held, oldest first: all of them, but while the first sweep
    /// on a screen is under way (drawn from the left) or a rolling screen
    /// first fills (drawn right-aligned, the newest at the right edge).
    pub points: u16,
    /// Points of the sweep under way, when the screen is mid-sweep (the
    /// rest, up to `points`, are the last sweep's); `points` when it shows
    /// whole sweeps, or rolls.
    pub swept: u16,
    /// Seconds across the screen.
    pub span: f32,
    /// Whether the sweep started on a trigger (rather than waiting too
    /// long for one, or running free).
    pub triggered: bool,
    /// Whether it is input a against input b.
    pub xy: bool,
    /// Each input's lowest at each point.
    pub low: [[f32; TRACE_POINTS]; 2],
    /// Each input's highest at each point.
    pub high: [[f32; TRACE_POINTS]; 2],
}

impl Trace {
    /// No trace: a readout's.
    pub const NONE: Self = Self {
        capacity: 0,
        points: 0,
        swept: 0,
        span: 0.0,
        triggered: false,
        xy: false,
        low: [[0.0; TRACE_POINTS]; 2],
        high: [[0.0; TRACE_POINTS]; 2],
    };
}

/// What a scope or readout hands the engine: plain data of a fixed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capture {
    /// Its knobs as it read them for this capture, in catalogue order
    /// (moved by any cable in their jacks); zero past its own.
    pub knobs: [f32; 4],
    /// Each input's latest reading: a scope's a and b, a readout's input
    /// first.
    pub inputs: [Reading; 2],
    /// The scope's trace; none for a readout.
    pub trace: Trace,
}

impl Capture {
    /// An empty capture.
    pub const NONE: Self = Self {
        knobs: [0.0; 4],
        inputs: [Reading::NONE; 2],
        trace: Trace::NONE,
    };
}

/// What a run of periods repeats as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repeats {
    /// A wave: each crossing of its middle, or each pair of them when they
    /// alternate long and short, is a cycle.
    Wave,
    /// A gate: each rise is a pulse, alternating gaps are a swung pair of
    /// pulses, and gaps of whole numbers of the shortest have rests in
    /// them.
    Pulses,
}

/// The latest periods, newest last.
#[derive(Debug, Clone, Copy, PartialEq)]
struct History {
    periods: [f64; HISTORY],
    count: usize,
    next: usize,
}

impl History {
    const EMPTY: Self = Self {
        periods: [0.0; HISTORY],
        count: 0,
        next: 0,
    };

    fn push(&mut self, period: f64) {
        self.periods[self.next] = period;
        self.next = (self.next + 1) % HISTORY;
        self.count = (self.count + 1).min(HISTORY);
    }

    /// The `age`th newest period (0 the newest).
    const fn nth(&self, age: usize) -> f64 {
        self.periods[(self.next + HISTORY - 1 - age) % HISTORY]
    }

    /// The longest period kept.
    fn longest(&self) -> Option<f64> {
        (0..self.count).map(|age| self.nth(age)).reduce(f64::max)
    }

    /// How many of the newest periods to read: those spanning about
    /// `frames`, at least one, and at least `least` when there are that
    /// many (four for a wave, so pairs can be told; eight for a gate, so
    /// swung pairs can be told from rests).
    fn span(&self, frames: f64, least: usize) -> usize {
        let mut total = 0.0;
        let mut used = 0;
        while used < self.count {
            let period = self.nth(used);
            if used >= least && total + period > frames {
                break;
            }
            total += period;
            used += 1;
        }
        used
    }

    /// Frames per cycle (a wave's) or per pulse (a gate's) over about the
    /// newest `frames`, and whether it is steady; none with no periods.
    fn estimate(&self, frames: f64, repeats: Repeats) -> Option<(f64, bool)> {
        let least = match repeats {
            Repeats::Wave => 4,
            Repeats::Pulses => 8,
        };
        let used = self.span(frames, least);
        if used == 0 {
            return None;
        }
        let singles = (0..used).map(|age| self.nth(age));
        let (total, steady) = agreeing(singles);
        // At most HISTORY: exact.
        let mean = total / used as f64;
        if steady || used < 4 {
            // Steady takes three periods that agree: two can by chance.
            return (mean > 0.0).then_some((mean, steady && used >= 3));
        }
        // Alternating long and short: each pair is one cycle (a wave), or
        // two pulses (a swung gate).
        let pairs = used / 2;
        let (sum, paired) =
            agreeing((0..pairs).map(|pair| self.nth(2 * pair) + self.nth(2 * pair + 1)));
        if paired {
            let per_pair = sum / pairs as f64;
            return Some(match repeats {
                Repeats::Wave => (per_pair, true),
                Repeats::Pulses => (per_pair / 2.0, true),
            });
        }
        if repeats == Repeats::Pulses {
            // Rests: every gap a whole number of the shortest.
            let shortest = (0..used)
                .map(|age| self.nth(age))
                .fold(f64::INFINITY, f64::min);
            if shortest > 0.0 {
                let mut slots = 0.0;
                let whole = (0..used).all(|age| {
                    let ratio = self.nth(age) / shortest;
                    slots += ratio.round();
                    (ratio - ratio.round()).abs() <= WHOLE_SPREAD * ratio.round()
                });
                if whole && slots > 0.0 {
                    return Some((total / slots, true));
                }
            }
        }
        (mean > 0.0).then_some((mean, false))
    }

    const fn clear(&mut self) {
        self.count = 0;
        self.next = 0;
    }

    /// After a wave's new period: when its two newest periods agree with
    /// each other and are more than half as long again (or as short) as
    /// the one before, its rate has changed, and only those two are kept.
    /// (A wave whose periods alternate long and short has newest two that
    /// disagree, and keeps them all.)
    fn follow_change(&mut self) {
        if self.count < 3 {
            return;
        }
        let (newest, before, older) = (self.nth(0), self.nth(1), self.nth(2));
        let agree = (newest - before).abs() <= CHANGE_AGREE * newest.max(before);
        let ratio = newest / older;
        if agree && !(1.0 / CHANGE_RATIO..=CHANGE_RATIO).contains(&ratio) {
            self.clear();
            self.push(before);
            self.push(newest);
        }
    }

    /// Whether `open` frames without a crossing run past the longest period
    /// kept (by more than [`STEADY_SPREAD`]).
    fn overdue(&self, open: f64) -> bool {
        self.longest()
            .is_some_and(|longest| open > longest * (1.0 + STEADY_SPREAD))
    }
}

/// The sum of `periods`, and whether they all lie within
/// [`STEADY_SPREAD`] of their mean.
fn agreeing(periods: impl Iterator<Item = f64> + Clone) -> (f64, bool) {
    let (mut total, mut count) = (0.0, 0_u32);
    let (mut shortest, mut longest) = (f64::INFINITY, 0.0_f64);
    for period in periods {
        total += period;
        count += 1;
        shortest = shortest.min(period);
        longest = longest.max(period);
    }
    let mean = total / f64::from(count.max(1));
    (
        total,
        count >= 2 && longest - shortest <= STEADY_SPREAD * mean,
    )
}

/// Times a signal's repeats: rising crossings of its middle, with
/// hysteresis, the middle and the swing taken from its latest full cycle
/// and the cycle under way.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Period {
    previous: Option<f32>,
    armed: bool,
    last_cross: Option<f64>,
    /// When the cycle under way started (a crossing, or a fresh start).
    opened: u64,
    cycle_low: f32,
    cycle_high: f32,
    full_low: f32,
    full_high: f32,
    history: History,
}

impl Period {
    const NEW: Self = Self {
        previous: None,
        armed: false,
        last_cross: None,
        opened: 0,
        cycle_low: f32::INFINITY,
        cycle_high: f32::NEG_INFINITY,
        full_low: f32::INFINITY,
        full_high: f32::NEG_INFINITY,
        history: History::EMPTY,
    };

    /// Take sample `x`, at `frame`; after `longest` frames without a
    /// crossing, what is known is let go.
    fn feed(&mut self, frame: u64, x: f32, longest: f64) {
        let previous = self.previous.replace(x);
        if frame.saturating_sub(self.opened) as f64 > longest {
            // Nothing has crossed for too long: it stopped, or changed.
            self.history.clear();
            self.last_cross = None;
            self.full_low = f32::INFINITY;
            self.full_high = f32::NEG_INFINITY;
            self.cycle_low = x;
            self.cycle_high = x;
            self.opened = frame;
            self.armed = false;
        }
        self.cycle_low = self.cycle_low.min(x);
        self.cycle_high = self.cycle_high.max(x);
        let low = self.cycle_low.min(self.full_low);
        let high = self.cycle_high.max(self.full_high);
        let swing = high - low;
        if swing < LEAST_SWING {
            return;
        }
        let middle = 0.5 * (low + high);
        let band = HYSTERESIS * 0.5 * swing;
        if x < middle - band {
            self.armed = true;
            return;
        }
        let fire = middle + band;
        if !(self.armed && x >= fire) {
            return;
        }
        // Between the two samples, where it passed the threshold.
        let between = previous.map_or(0.0, |before| {
            let rise = x - before;
            if rise > 0.0 {
                ((fire - before) / rise).clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        let at = frame as f64 - 1.0 + f64::from(between);
        if let Some(last) = self.last_cross {
            self.history.push(at - last);
            self.history.follow_change();
        }
        self.last_cross = Some(at);
        self.armed = false;
        self.full_low = self.cycle_low;
        self.full_high = self.cycle_high;
        self.cycle_low = x;
        self.cycle_high = x;
        self.opened = frame;
    }

    /// Frames per cycle over about `average` frames, and whether it is
    /// steady, if it repeats (a cycle gone on too long has already let its
    /// periods go).
    fn period(&self, average: f64) -> Option<(f64, bool)> {
        self.history.estimate(average, Repeats::Wave)
    }

    /// Whether, at `frame`, the cycle under way has run past the longest
    /// period kept.
    fn paused(&self, frame: u64) -> bool {
        self.last_cross.is_some()
            && self
                .history
                .overdue(frame.saturating_sub(self.opened) as f64)
    }
}

/// Times a gate: its rising edges past 0.5, and how long it is high.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pulses {
    edge: Edge,
    last_rise: Option<u64>,
    high_frames: u64,
    /// How long the latest whole pulse was high, in frames.
    pulse: Option<u64>,
    history: History,
}

impl Pulses {
    const NEW: Self = Self {
        edge: Edge::new(),
        last_rise: None,
        high_frames: 0,
        pulse: None,
        history: History::EMPTY,
    };

    fn feed(&mut self, frame: u64, x: f32, longest: f64) {
        if self.edge.rising(x) {
            if let Some(last) = self.last_rise {
                let period = frame.saturating_sub(last);
                if period > 0 {
                    self.history.push(period as f64);
                    self.pulse = Some(self.high_frames);
                }
            }
            self.last_rise = Some(frame);
            self.high_frames = 0;
        }
        if self.edge.is_high() {
            self.high_frames = self.high_frames.saturating_add(1);
        }
        let quiet = self.last_rise.map_or(0, |last| frame.saturating_sub(last)) as f64;
        if quiet > longest {
            // No rise for too long: the gate has stopped.
            self.history.clear();
            self.pulse = None;
            self.last_rise = None;
        }
    }

    /// Whether, at `frame`, it has gone without a rise for longer than the
    /// longest gap kept.
    fn paused(&self, frame: u64) -> bool {
        self.last_rise
            .is_some_and(|last| self.history.overdue(frame.saturating_sub(last) as f64))
    }
}

/// Watches one signal (see the module's notes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meter {
    sample_rate: f32,
    window: u32,
    frame: u64,
    connected: bool,
    seen: u32,
    count: u32,
    faults: u32,
    faults_since: u32,
    sum: f64,
    squares: f64,
    low: f32,
    high: f32,
    peak: f32,
    rising: u32,
    falling: u32,
    held: u32,
    previous: Option<f32>,
    last: f32,
    min: f32,
    max: f32,
    period: Period,
    pulses: Pulses,
    latest: Reading,
}

impl Meter {
    /// A meter for a stream at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 1_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            sample_rate,
            // A tenth of a few hundred kHz at most: exact.
            window: (sample_rate * WINDOW_SECONDS).round().max(1.0) as u32,
            frame: 0,
            connected: false,
            seen: 0,
            count: 0,
            faults: 0,
            faults_since: 0,
            sum: 0.0,
            squares: 0.0,
            low: f32::INFINITY,
            high: f32::NEG_INFINITY,
            peak: 0.0,
            rising: 0,
            falling: 0,
            held: 0,
            previous: None,
            last: 0.0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            period: Period::NEW,
            pulses: Pulses::NEW,
            latest: Reading::NONE,
        }
    }

    /// The latest window's reading.
    #[must_use]
    pub const fn latest(&self) -> &Reading {
        &self.latest
    }

    /// Say whether a cable is in (the reading says so).
    pub const fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    /// Forget the lowest and highest, and the faults, since the last
    /// clear.
    pub const fn clear_extremes(&mut self) {
        self.min = f32::INFINITY;
        self.max = f32::NEG_INFINITY;
        self.faults_since = 0;
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        let connected = self.connected;
        *self = Self::new(self.sample_rate);
        self.connected = connected;
    }

    /// Start afresh on a new signal, its input re-plugged: what was
    /// measured of the old one is let go, but for the lowest, the highest
    /// and the faults since the last clear (which only a clear lets go).
    pub fn rewire(&mut self) {
        let kept = (self.connected, self.min, self.max, self.faults_since);
        *self = Self::new(self.sample_rate);
        (self.connected, self.min, self.max, self.faults_since) = kept;
    }

    /// Whether a window has closed since it started, was reset or was
    /// re-plugged: whether [`Self::latest`] is of the signal it has now.
    #[must_use]
    pub const fn fresh(&self) -> bool {
        self.latest.frames > 0 || self.latest.faults > 0
    }

    /// The longest period, in frames.
    fn longest(&self) -> f64 {
        f64::from(self.sample_rate * LONGEST_PERIOD_SECONDS)
    }

    /// Take one sample. Returns whether a window closed with it (its
    /// reading is then [`Self::latest`]).
    pub fn feed(&mut self, sample: f32) -> bool {
        self.frame = self.frame.wrapping_add(1);
        if sample.is_finite() {
            self.take(sample);
        } else {
            self.faults = self.faults.saturating_add(1);
            self.faults_since = self.faults_since.saturating_add(1);
        }
        self.seen += 1;
        if self.seen < self.window {
            return false;
        }
        self.close();
        true
    }

    fn take(&mut self, x: f32) {
        self.count += 1;
        self.sum += f64::from(x);
        self.squares = f64::from(x).mul_add(f64::from(x), self.squares);
        self.low = self.low.min(x);
        self.high = self.high.max(x);
        self.peak = self.peak.max(x.abs());
        self.min = self.min.min(x);
        self.max = self.max.max(x);
        if let Some(before) = self.previous {
            if x > before {
                self.rising += 1;
            } else if x < before {
                self.falling += 1;
            }
        }
        let (low, high) = (self.latest.low, self.latest.high);
        let reach = HELD_REACH * (high - low);
        if self.latest.frames > 0
            && high - low >= LEAST_SWING
            && (x <= low + reach || x >= high - reach)
        {
            self.held += 1;
        }
        self.previous = Some(x);
        self.last = x;
        let longest = self.longest();
        self.period.feed(self.frame, x, longest);
        self.pulses.feed(self.frame, x, longest);
    }

    /// Close the window: its reading becomes the latest.
    fn close(&mut self) {
        let rate = f64::from(self.sample_rate);
        let count = f64::from(self.count);
        let measured = self.count > 0;
        let (hz, steady) = self
            .period
            .period(rate * AVERAGE_SECONDS)
            .map_or((0.0, false), |(frames, steady)| (rate / frames, steady));
        let paused = hz > 0.0 && self.period.paused(self.frame);
        let per_pulse = self
            .pulses
            .history
            .estimate(rate * AVERAGE_SECONDS, Repeats::Pulses);
        let gate_hz = per_pulse.map_or(0.0, |(frames, _)| rate / frames);
        let duty = match (per_pulse, self.pulses.pulse) {
            (Some((frames, _)), Some(high)) => (high as f64 / frames).min(1.0) as f32,
            _ => -1.0,
        };
        let moved = self.rising + self.falling;
        self.latest = Reading {
            connected: self.connected,
            frames: self.count,
            faults: self.faults,
            faults_since: self.faults_since,
            last: self.last,
            low: if measured { self.low } else { 0.0 },
            high: if measured { self.high } else { 0.0 },
            mean: if measured {
                finite((self.sum / count) as f32)
            } else {
                0.0
            },
            rms: if measured {
                finite((self.squares / count).sqrt() as f32)
            } else {
                0.0
            },
            peak: self.peak,
            hz: finite(hz as f32),
            steady: steady && !paused,
            paused,
            rising: if moved > 0 {
                self.rising as f32 / moved as f32
            } else {
                0.0
            },
            held: if measured {
                self.held as f32 / self.count as f32
            } else {
                0.0
            },
            gate: GateReading {
                hz: finite(gate_hz as f32),
                duty,
                high: self.pulses.edge.is_high(),
                paused: gate_hz > 0.0 && self.pulses.paused(self.frame),
            },
            min: self.min,
            max: self.max,
        };
        self.seen = 0;
        self.count = 0;
        self.faults = 0;
        self.sum = 0.0;
        self.squares = 0.0;
        self.low = f32::INFINITY;
        self.high = f32::NEG_INFINITY;
        self.peak = 0.0;
        self.rising = 0;
        self.falling = 0;
        self.held = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: f32 = 48_000.0;

    /// Feed `meter` `frames` samples of `signal`, returning the last
    /// reading closed.
    fn feed(meter: &mut Meter, frames: usize, mut signal: impl FnMut(usize) -> f32) -> Reading {
        meter.set_connected(true);
        for frame in 0..frames {
            meter.feed(signal(frame));
        }
        *meter.latest()
    }

    fn sine(hz: f64, amplitude: f64, offset: f64) -> impl Fn(usize) -> f32 {
        move |frame| {
            amplitude.mul_add((TAU * hz * frame as f64 / f64::from(RATE)).sin(), offset) as f32
        }
    }

    #[test]
    fn a_sine_reads_its_frequency_level_and_offset() {
        for hz in [27.5, 110.0, 440.0, 1_000.0, 3_520.0, 12_000.0] {
            let mut meter = Meter::new(RATE);
            let reading = feed(&mut meter, 48_000, sine(hz, 0.5, 0.0));
            assert!(
                (f64::from(reading.hz) - hz).abs() < hz * 1e-4,
                "{hz}: {reading:?}"
            );
            assert!(reading.steady, "{hz}");
            // A window holding part of a cycle leans its mean and RMS by
            // at most that part's share of the wave.
            let lean = 0.005 + 0.5 / (std::f64::consts::PI * hz * f64::from(WINDOW_SECONDS));
            let rms = f64::from(reading.rms) - 0.5 / 2.0_f64.sqrt();
            assert!(rms.abs() < lean, "{hz}: {reading:?}");
            assert!((reading.peak - 0.5).abs() < 0.01, "{hz}");
            assert!(f64::from(reading.mean).abs() < lean, "{hz}: {reading:?}");
            assert!((reading.min + 0.5).abs() < 0.01 && (reading.max - 0.5).abs() < 0.01);
            assert!((0.3..0.55).contains(&reading.held), "{hz}: {reading:?}");
        }
        // An offset moves the mean, not the frequency.
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 24_000, sine(220.0, 0.25, 0.6));
        assert!((reading.mean - 0.6).abs() < 0.005, "{reading:?}");
        assert!((reading.hz - 220.0).abs() < 0.05, "{reading:?}");
        assert!((reading.low - 0.35).abs() < 0.005 && (reading.high - 0.85).abs() < 0.005);
    }

    #[test]
    fn slow_waves_are_timed_too_and_stop_when_they_stop() {
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 48_000 * 9, sine(0.5, 1.0, 0.0));
        assert!((reading.hz - 0.5).abs() < 1e-3, "{reading:?}");
        assert!(reading.steady, "{reading:?}");
        assert!(!reading.paused, "{reading:?}");
        // Held still, at once it has paused: its last rate, not steady.
        let reading = feed(&mut meter, 48_000 * 3, |_| 0.25);
        assert!((reading.hz - 0.5).abs() < 1e-3, "{reading:?}");
        assert!(reading.paused && !reading.steady, "{reading:?}");
        // After the longest period measured, it no longer repeats at all.
        let reading = feed(&mut meter, 48_000 * 18, |_| 0.25);
        assert!(reading.hz.abs() < f32::EPSILON, "{reading:?}");
        assert!(!reading.paused, "{reading:?}");
        assert!((reading.last - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn harmonics_and_noise_do_not_fool_the_crossings() {
        // A bright saw: one crossing a cycle however steep its edge.
        let saw = |frame: usize| {
            let phase = (frame as f64 * 330.0 / f64::from(RATE)).fract();
            phase.mul_add(2.0, -1.0) as f32
        };
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 24_000, saw);
        assert!((reading.hz - 330.0).abs() < 0.1, "{reading:?}");
        assert!(reading.rising > 0.95, "a rising saw rises: {reading:?}");
        assert!(reading.held < 0.3, "{reading:?}");
        // A sine with a little noise on it still reads true.
        let mut rng = crate::dsp::Rng::new(9);
        let noisy: Vec<f32> = (0..24_000)
            .map(|frame| 0.02_f32.mul_add(rng.bipolar(), sine(440.0, 0.8, 0.0)(frame)))
            .collect();
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, noisy.len(), |frame| noisy[frame]);
        assert!((reading.hz - 440.0).abs() < 0.5, "{reading:?}");
        // Noise alone does not repeat steadily.
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 24_000, |_| rng.bipolar());
        assert!(!reading.steady, "{reading:?}");
    }

    #[test]
    fn a_square_is_held_at_its_extremes() {
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 24_000, |frame| {
            if frame % 120 < 60 { 0.4 } else { -0.4 }
        });
        assert!(reading.held > 0.99, "{reading:?}");
        assert!((reading.hz - 400.0).abs() < 1e-3, "{reading:?}");
        assert!((reading.rising - 0.5).abs() < 0.01, "{reading:?}");
    }

    #[test]
    fn a_slow_lfo_stepping_its_rate_reads_true_within_two_new_periods() {
        // 2 Hz for three seconds, then 0.2 Hz: each period five seconds. (In
        // between it reads as paused at 2 Hz.)
        let mut meter = Meter::new(RATE);
        let step = 48_000 * 3;
        let mut phase = 0.0_f64;
        let mut wave = move |frame: usize| {
            let hz = if frame < step { 2.0 } else { 0.2 };
            phase = (phase + hz / f64::from(RATE)).fract();
            (std::f64::consts::TAU * phase).sin() as f32
        };
        let reading = feed(&mut meter, step, &mut wave);
        assert!(
            (reading.hz - 2.0).abs() < 1e-3 && reading.steady,
            "{reading:?}"
        );
        // Two whole new periods and a little: 0.2 Hz; three: steady too.
        let mut frame = step;
        let mut reading = Reading::NONE;
        let mut at_two = Reading::NONE;
        while frame < step + 48_000 * 16 {
            if meter.feed(wave(frame)) {
                reading = *meter.latest();
            }
            frame += 1;
            if frame == step + 48_000 * 11 {
                at_two = reading;
            }
        }
        assert!((at_two.hz - 0.2).abs() < 1e-3, "{at_two:?}");
        assert!(!at_two.paused, "{at_two:?}");
        assert!((reading.hz - 0.2).abs() < 1e-3, "{reading:?}");
        assert!(reading.steady, "{reading:?}");
        // A glide is followed as it goes, never stuck on the old rate.
        let mut meter = Meter::new(RATE);
        let mut phase = 0.0_f64;
        let mut last = Reading::NONE;
        for frame in 0..48_000 * 4 {
            let hz = 200.0 + 100.0 * f64::from(frame) / (48_000.0 * 4.0);
            phase = (phase + hz / f64::from(RATE)).fract();
            if meter.feed((std::f64::consts::TAU * phase).sin() as f32) {
                last = *meter.latest();
            }
        }
        assert!((last.hz - 300.0).abs() < 3.0, "{last:?}");
    }

    #[test]
    fn a_gate_that_stops_says_it_has_paused() {
        let mut meter = Meter::new(RATE);
        feed(&mut meter, 48_000 * 2, |frame| {
            if frame % 6_000 < 3_000 { 1.0 } else { 0.0 }
        });
        let reading = feed(&mut meter, 9_600, |_| 0.0);
        assert!((reading.gate.hz - 8.0).abs() < 1e-3, "{reading:?}");
        assert!(reading.gate.paused, "{reading:?}");
    }

    #[test]
    fn a_strong_second_harmonic_is_not_an_octave_up() {
        // It crosses its middle twice a cycle, long then short: each pair
        // is one cycle.
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 24_000, |frame| {
            let x = std::f64::consts::TAU * 220.0 * frame as f64 / f64::from(RATE);
            0.9_f64.mul_add((2.0 * x).sin(), x.sin()) as f32
        });
        assert!((reading.hz - 220.0).abs() < 0.1, "{reading:?}");
        assert!(reading.steady, "{reading:?}");
    }

    #[test]
    fn a_swung_clock_and_a_gate_with_rests_keep_their_rate() {
        // Swing 0.75 at 120 BPM sixteenths: pairs of 7 500 and 4 500
        // frames, a pulse every 6 000 on average.
        let mut meter = Meter::new(RATE);
        let mut readings = Vec::new();
        for frame in 0..48_000 * 3 {
            let place = frame % 12_000;
            let high = place < 1_000 || (7_500..8_500).contains(&place);
            if meter.feed(if high { 1.0 } else { 0.0 }) {
                readings.push(*meter.latest());
            }
        }
        for reading in &readings[10..] {
            assert!((reading.gate.hz - 8.0).abs() < 1e-3, "{reading:?}");
        }
        // Sixteenths with rests (the second and fifth of each eight
        // silent): it is still a sixteenth clock, never let go.
        let mut meter = Meter::new(RATE);
        let mut readings = Vec::new();
        for frame in 0..48_000 * 4 {
            let step = frame / 6_000 % 8;
            let high = frame % 6_000 < 3_000 && step != 1 && step != 4;
            if meter.feed(if high { 1.0 } else { 0.0 }) {
                readings.push(*meter.latest());
            }
        }
        for reading in &readings[10..] {
            assert!((reading.gate.hz - 8.0).abs() < 1e-3, "{reading:?}");
            assert!((reading.gate.duty - 0.5).abs() < 1e-3, "{reading:?}");
        }
    }

    #[test]
    fn a_gate_reads_its_rate_and_duty() {
        // 120 BPM quarter notes, high a quarter of each.
        let period = 24_000;
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 48_000 * 3, |frame| {
            if frame % period < period / 4 {
                1.0
            } else {
                0.0
            }
        });
        assert!((reading.gate.hz - 2.0).abs() < 1e-4, "{reading:?}");
        assert!((reading.gate.duty - 0.25).abs() < 1e-3, "{reading:?}");
        // A sixteenth-note clock at 120 BPM, half high.
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 48_000, |frame| {
            if frame % 6_000 < 3_000 { 1.0 } else { 0.0 }
        });
        assert!((reading.gate.hz - 8.0).abs() < 1e-4, "{reading:?}");
        assert!((reading.gate.duty - 0.5).abs() < 1e-3, "{reading:?}");
        // A gate held low has paused, and after the longest gap measured
        // stops being timed.
        let reading = feed(&mut meter, 48_000, |_| 0.0);
        assert!(reading.gate.paused && !reading.gate.high, "{reading:?}");
        let reading = feed(&mut meter, 48_000 * 20, |_| 0.0);
        assert!(reading.gate.hz.abs() < f32::EPSILON && reading.gate.duty < 0.0);
        assert!(!reading.gate.paused);
    }

    #[test]
    fn non_numbers_are_faults_not_values() {
        let mut meter = Meter::new(RATE);
        let reading = feed(&mut meter, 4_800, |frame| {
            if frame % 2 == 0 { f32::NAN } else { 0.5 }
        });
        assert_eq!(reading.faults, 2_400);
        assert_eq!(reading.faults_since, 2_400);
        assert_eq!(reading.frames, 2_400);
        assert!((reading.mean - 0.5).abs() < f32::EPSILON);
        assert!((reading.max - 0.5).abs() < f32::EPSILON);
        let reading = feed(&mut meter, 4_800, |_| f32::INFINITY);
        assert_eq!(reading.faults, 4_800);
        assert_eq!(reading.faults_since, 7_200, "they add up until cleared");
        assert_eq!(reading.frames, 0);
        for value in [reading.mean, reading.rms, reading.low, reading.high] {
            assert!(value.is_finite() && value.abs() < f32::EPSILON);
        }
        // What was measured before the fault is still the extremes.
        assert!((reading.max - 0.5).abs() < f32::EPSILON);
        // A clean window has no faults of its own, but the total stays.
        let reading = feed(&mut meter, 4_800, |_| 0.1);
        assert_eq!((reading.faults, reading.faults_since), (0, 7_200));
        meter.clear_extremes();
        let reading = feed(&mut meter, 4_800, |_| 0.1);
        assert_eq!(reading.faults_since, 0);
    }

    #[test]
    fn extremes_hold_until_cleared() {
        let mut meter = Meter::new(RATE);
        feed(
            &mut meter,
            4_800,
            |frame| if frame == 7 { 3.0 } else { -0.25 },
        );
        let reading = feed(&mut meter, 4_800, |_| 0.1);
        assert!((reading.max - 3.0).abs() < f32::EPSILON);
        assert!((reading.min + 0.25).abs() < f32::EPSILON);
        meter.clear_extremes();
        let reading = feed(&mut meter, 4_800, |_| 0.1);
        assert!((reading.max - 0.1).abs() < f32::EPSILON);
        assert!((reading.min - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn windows_close_every_tenth_of_a_second() {
        let mut meter = Meter::new(RATE);
        let closed = (0..48_000).filter(|_| meter.feed(0.0)).count();
        assert_eq!(closed, 10);
        // A nonsense rate falls back to a usable one.
        let mut meter = Meter::new(f32::NAN);
        let closed = (0..48_000).filter(|_| meter.feed(0.0)).count();
        assert_eq!(closed, 10);
        meter.reset();
        assert_eq!(*meter.latest(), Reading::NONE);
    }
}
