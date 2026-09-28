//! Where the engine runs: a cpal output stream, or a headless loop.
//!
//! The headless loop is how the daemon runs with no audio device (tests,
//! servers, a machine whose device is gone): it renders in step with the
//! desk when the desk paces it, and in real time on its own timer
//! otherwise, so the clock, the glides, the desk link and the listening all
//! carry on, and the rendered sound goes to the desk when the wall is
//! plugged in, or nowhere.
//!
//! The device plays at whatever rate it is already set to, so a DAC run
//! bit-perfect at its owner's chosen rate is never switched; asking for a
//! rate is an explicit override. The stream asks for a fixed buffer (512
//! frames, 1024 above 96 kHz) rather than the backend's default, and falls
//! back to the default if the device refuses. Floating-point devices get
//! the wall's samples as they are; integer devices get them rounded with
//! triangular dither of one least significant bit, so quiet passages fade
//! into noise rather than into distortion.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::dsp::Rng;
use kazoo_core::ipc::link::DeskOwes;

use crate::engine::Engine;

/// Frames the headless loop renders at a time.
const HEADLESS_BLOCK: usize = 256;

/// Lead, beyond the desk's usual, that the headless loop asks the desk to
/// give it: a timer wakes later than a device callback on a busy machine,
/// and a block that reaches the desk after its time makes the desk place
/// the stream afresh, which is heard as a jump.
const HEADLESS_SLACK_SECONDS: f64 = 0.04;

/// Longest the headless loop sleeps while it is ahead of the desk, so it
/// hears the desk's latest pace soon after it arrives.
const PACED_NAP: Duration = Duration::from_millis(2);

/// Shortest sleep while ahead of the desk.
const SHORTEST_NAP: Duration = Duration::from_micros(250);

/// After a stall long enough that the desk has played past the wall, how
/// long a pace must have arrived after the catching-up block for it to say
/// where the desk placed the stream afresh (the desk paces every 5 ms).
const REPLACE_SETTLE: Duration = Duration::from_millis(10);

/// [`HEADLESS_SLACK_SECONDS`] at `rate`, in frames.
#[must_use]
pub fn headless_lead_frames(rate: u32) -> u32 {
    // Rates are at most a few hundred kHz: a few thousand frames.
    (f64::from(rate) * HEADLESS_SLACK_SECONDS).round() as u32
}

/// Samples the device callback converts at a time, for devices that do not
/// take f32.
const CONVERT_BLOCK: usize = 8_192;

/// The buffer asked for, in frames, up to 96 kHz.
const BUFFER_FRAMES: u32 = 512;

/// The buffer asked for above 96 kHz: the same few milliseconds' headroom.
const FAST_BUFFER_FRAMES: u32 = 1_024;

/// How the daemon makes sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    /// The default output device, falling back to the headless loop while
    /// it cannot be opened.
    Device {
        /// The rate to switch the device to; `None` (the default) plays at
        /// whatever rate it is already set to, so the owner's DAC setting is
        /// never changed behind their back.
        sample_rate: Option<u32>,
    },
    /// No device: the headless loop only, at this sample rate.
    Headless {
        /// Frames per second.
        sample_rate: u32,
    },
}

/// What the device's error callback has seen.
#[derive(Debug, Default)]
pub struct StreamHealth {
    /// The stream is gone and must be rebuilt.
    pub failed: AtomicBool,
    /// Underruns and overruns.
    pub glitches: AtomicU64,
    /// Other errors the backend reported.
    pub errors: AtomicU64,
}

/// A device ready to play: its config and sample format.
pub struct Device {
    handle: cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    /// The buffer the device can take, if it says.
    buffers: cpal::SupportedBufferSize,
    name: String,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("sample_rate", &self.config.sample_rate)
            .field("channels", &self.config.channels)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl Device {
    /// The default output device at the rate it is set to.
    ///
    /// # Errors
    ///
    /// Fails if there is no output device, or it cannot say how it plays.
    pub fn open_default() -> Result<Self, String> {
        Self::open(None)
    }

    /// The default output device, at `sample_rate` if one is asked for
    /// (which switches the device to it), else at the rate it is set to.
    ///
    /// # Errors
    ///
    /// Fails if there is no output device, it cannot say how it plays, or
    /// it cannot play at the rate asked for.
    pub fn open(sample_rate: Option<u32>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let supported = match sample_rate {
            None => device
                .default_output_config()
                .map_err(|err| format!("the output device will not say how it plays: {err}"))?,
            Some(rate) => at_rate(&device, rate)?,
        };
        let format = supported.sample_format();
        let buffers = *supported.buffer_size();
        let config: cpal::StreamConfig = supported.into();
        if config.channels == 0 {
            return Err("the output device has no channels".to_string());
        }
        let name = match device.description() {
            Ok(description) => description.name().to_string(),
            Err(err) => format!("an unnamed device ({err})"),
        };
        Ok(Self {
            handle: device,
            config,
            format,
            buffers,
            name,
        })
    }

    /// Frames per second.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// The device's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Start playing `engine` on this device.
    ///
    /// # Errors
    ///
    /// Fails if the sample format is not one the wall can write, or the
    /// stream cannot be built or started.
    pub fn play(&self, engine: Engine, health: &Arc<StreamHealth>) -> Result<cpal::Stream, String> {
        // The engine moves into the stream's callback, so a refused fixed
        // buffer cannot be retried with the same engine: ask the device
        // first whether it takes the buffer, and only ask for one it does.
        let mut config = self.config.clone();
        config.buffer_size = self.buffer_size();
        let stream = match self.format {
            cpal::SampleFormat::F32 => self.build_f32(&config, engine, health),
            cpal::SampleFormat::I16 => self.build::<i16>(&config, engine, health),
            cpal::SampleFormat::I32 => self.build::<i32>(&config, engine, health),
            cpal::SampleFormat::U16 => self.build::<u16>(&config, engine, health),
            cpal::SampleFormat::F64 => self.build::<f64>(&config, engine, health),
            other => {
                return Err(format!(
                    "the output device plays {other:?} samples, which the wall cannot write"
                ));
            }
        }
        .map_err(|err| format!("the output stream could not be built: {err}"))?;
        stream
            .play()
            .map_err(|err| format!("the output stream would not start: {err}"))?;
        Ok(stream)
    }

    /// The fixed buffer to ask for, when the device says it can take it;
    /// otherwise the backend's default.
    fn buffer_size(&self) -> cpal::BufferSize {
        let wanted = if self.config.sample_rate > 96_000 {
            FAST_BUFFER_FRAMES
        } else {
            BUFFER_FRAMES
        };
        match self.buffers {
            cpal::SupportedBufferSize::Range { min, max } if min <= max => {
                cpal::BufferSize::Fixed(wanted.clamp(min, max))
            }
            cpal::SupportedBufferSize::Range { .. } | cpal::SupportedBufferSize::Unknown => {
                cpal::BufferSize::Default
            }
        }
    }

    fn build_f32(
        &self,
        config: &cpal::StreamConfig,
        mut engine: Engine,
        health: &Arc<StreamHealth>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError> {
        let channels = usize::from(config.channels);
        self.handle.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data, channels),
            error_callback(health),
            None,
        )
    }

    fn build<T>(
        &self,
        config: &cpal::StreamConfig,
        mut engine: Engine,
        health: &Arc<StreamHealth>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: cpal::SizedSample + Quantise,
    {
        let channels = usize::from(config.channels);
        let mut scratch = vec![0.0_f32; CONVERT_BLOCK];
        let chunk = (CONVERT_BLOCK / channels).max(1) * channels;
        let mut dither = Dither::new();
        self.handle.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for out in data.chunks_mut(chunk) {
                    let rendered = &mut scratch[..out.len()];
                    engine.render(rendered, channels);
                    for (target, sample) in out.iter_mut().zip(rendered.iter()) {
                        *target = T::quantise(*sample, &mut dither);
                    }
                }
            },
            error_callback(health),
            None,
        )
    }
}

/// The supported config of `device` at `rate`: two channels and floats
/// where it offers them.
fn at_rate(device: &cpal::Device, rate: u32) -> Result<cpal::SupportedStreamConfig, String> {
    let configs = device
        .supported_output_configs()
        .map_err(|err| format!("the output device will not say how it plays: {err}"))?;
    configs
        .filter(|range| range.min_sample_rate() <= rate && rate <= range.max_sample_rate())
        .max_by_key(|range| {
            (
                range.sample_format() == cpal::SampleFormat::F32,
                range.channels() == 2,
                range.channels() >= 2,
            )
        })
        .map(|range| range.with_sample_rate(rate))
        .ok_or_else(|| format!("the output device cannot play at {rate} Hz"))
}

/// Triangular (TPDF) dither: the difference of two uniform random numbers,
/// spanning ±1 least significant bit, which makes the rounding error
/// independent of the signal.
#[derive(Debug)]
pub struct Dither {
    rng: Rng,
}

impl Dither {
    /// A dither source.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            rng: Rng::new(0x5EED_D1CE),
        }
    }

    /// The next dither value, from −1 to 1 (in least significant bits).
    pub fn sample(&mut self) -> f64 {
        f64::from(self.rng.unit()) - f64::from(self.rng.unit())
    }
}

impl Default for Dither {
    fn default() -> Self {
        Self::new()
    }
}

/// A device sample format the wall can write.
pub trait Quantise: Sized {
    /// `sample` (full scale ±1) in this format, dithered if it is an
    /// integer format.
    fn quantise(sample: f32, dither: &mut Dither) -> Self;
}

/// `sample` scaled to `scale` per unit, dithered by one step of `lsb`,
/// rounded and held to `min..=max`.
fn dithered(sample: f32, dither: &mut Dither, scale: f64, lsb: f64, min: f64, max: f64) -> f64 {
    let value = f64::from(kazoo_core::sanitize_sample(sample)) * scale;
    dither.sample().mul_add(lsb, value).round().clamp(min, max)
}

impl Quantise for i16 {
    fn quantise(sample: f32, dither: &mut Dither) -> Self {
        // Held to the i16 range: the cast is exact.
        dithered(sample, dither, 32_768.0, 1.0, -32_768.0, 32_767.0) as Self
    }
}

impl Quantise for u16 {
    fn quantise(sample: f32, dither: &mut Dither) -> Self {
        // Held to 0..=65535: the cast is exact.
        (dithered(sample, dither, 32_768.0, 1.0, -32_768.0, 32_767.0) + 32_768.0) as Self
    }
}

impl Quantise for i32 {
    fn quantise(sample: f32, dither: &mut Dither) -> Self {
        // A 32-bit device word carries 24 bits of converter at most: dither
        // at the 24-bit step. Held to the i32 range: the cast is exact.
        dithered(
            sample,
            dither,
            2_147_483_648.0,
            256.0,
            -2_147_483_648.0,
            2_147_483_647.0,
        ) as Self
    }
}

impl Quantise for f64 {
    fn quantise(sample: f32, _dither: &mut Dither) -> Self {
        Self::from(kazoo_core::sanitize_sample(sample))
    }
}

/// The stream's error callback: counts glitches and errors, and marks the
/// stream failed when the device goes or the stream is invalidated.
fn error_callback(health: &Arc<StreamHealth>) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let health = Arc::clone(health);
    move |err| match err {
        cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated => {
            health.failed.store(true, Ordering::Release);
        }
        cpal::StreamError::BufferUnderrun => {
            health.glitches.fetch_add(1, Ordering::Relaxed);
        }
        cpal::StreamError::BackendSpecific { .. } => {
            health.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The headless loop's own time: frames rendered against the time since
/// it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimerClock {
    started: Instant,
    rendered: u64,
}

impl TimerClock {
    const fn new(started: Instant) -> Self {
        Self {
            started,
            rendered: 0,
        }
    }

    /// How long to wait before the next block is due at `now`, or `None`
    /// when it is due. After a stall (a suspended machine) it carries on
    /// from now rather than rushing to catch up.
    fn wait(&mut self, now: Instant, rate: f64) -> Option<Duration> {
        // Frame counts are far below 2^53: exact.
        let due = Duration::from_secs_f64(self.rendered as f64 / rate);
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < due {
            return Some(due.saturating_sub(elapsed).min(Duration::from_millis(20)));
        }
        if elapsed > due + Duration::from_secs(1) {
            self.follow(now, rate);
        }
        None
    }

    /// Count time from `now` as though every frame rendered was due by
    /// then: while the desk paces the loop, so that when it stops, the
    /// timer carries on from where the desk left it.
    fn follow(&mut self, now: Instant, rate: f64) {
        let rendered = Duration::from_secs_f64(self.rendered as f64 / rate);
        if let Some(started) = now.checked_sub(rendered) {
            self.started = started;
        } else {
            // More rendered than the clock can reach back to: count afresh
            // from now, which is the same thing.
            self.started = now;
            self.rendered = 0;
        }
    }
}

/// What the headless loop does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Render a block now.
    Render,
    /// Sleep this long first.
    Sleep(Duration),
}

/// Decides when the headless loop renders: in step with a desk that paces
/// it, on its own timer otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pacer {
    timer: TimerClock,
    /// After the desk played past the wall: paces that arrived before this
    /// still place the stream where it was, and are not followed.
    settle_until: Option<Instant>,
    /// Whether it has waited once already for the desk to place the stream
    /// afresh, and is still further behind than the lead.
    settled: bool,
}

impl Pacer {
    const fn new(now: Instant) -> Self {
        Self {
            timer: TimerClock::new(now),
            settle_until: None,
            settled: false,
        }
    }

    /// The next step at `now` at `rate`, given what the desk says is owed.
    fn step(&mut self, owes: Option<DeskOwes>, now: Instant, rate: f64) -> Step {
        let Some(owes) = owes else {
            self.settle_until = None;
            self.settled = false;
            return self.timer.wait(now, rate).map_or(Step::Render, Step::Sleep);
        };
        self.timer.follow(now, rate);
        if self.settle_until.is_some_and(|until| owes.paced_at < until) {
            return Step::Sleep(PACED_NAP);
        }
        self.settle_until = None;
        if owes.frames > i64::try_from(owes.lead_frames).unwrap_or(i64::MAX) {
            if self.settled {
                // Waited once and still behind by more than the lead: the
                // desk took that block where it was (it arrived before the
                // desk's next buffer), so the stream stands, and the whole
                // debt is worth catching up.
                return Step::Render;
            }
            // Stalled so long the desk is playing past the next frame: it
            // will most likely place the stream afresh on this block, so
            // render one and wait to hear where, rather than pile the
            // whole debt up behind it.
            self.settled = true;
            self.settle_until = Some(now + REPLACE_SETTLE);
            return Step::Render;
        }
        self.settled = false;
        if owes.frames > 0 {
            return Step::Render;
        }
        Step::Sleep(
            Duration::from_secs_f64(owes.frames.unsigned_abs() as f64 / rate)
                .clamp(SHORTEST_NAP, PACED_NAP),
        )
    }

    /// Count a block of `frames` rendered.
    const fn rendered(&mut self, frames: u64) {
        self.timer.rendered += frames;
    }
}

/// The engine rendering with no audio device: in step with the desk when
/// it paces the wall, on the wall's own timer otherwise.
#[derive(Debug)]
pub struct Headless {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Headless {
    /// Start rendering `engine` at `sample_rate`: as far ahead as a desk
    /// that paces it places its stream, or in real time by its own timer.
    ///
    /// # Errors
    ///
    /// Fails if the thread cannot be started.
    pub fn start(mut engine: Engine, sample_rate: u32) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("kazoo-wall-render".to_string())
            .spawn(move || {
                let mut buffer = vec![0.0_f32; HEADLESS_BLOCK * 2];
                let rate = f64::from(sample_rate.max(1));
                let mut pacer = Pacer::new(Instant::now());
                while !running.load(Ordering::Acquire) {
                    let now = Instant::now();
                    match pacer.step(engine.desk_owes(now), now, rate) {
                        Step::Sleep(wait) => thread::sleep(wait),
                        Step::Render => {
                            engine.render(&mut buffer, 2);
                            // HEADLESS_BLOCK is 256: lossless.
                            pacer.rendered(HEADLESS_BLOCK as u64);
                        }
                    }
                }
            })?;
        Ok(Self {
            stop,
            join: Some(join),
        })
    }

    /// A loop that is not running (its thread could not start): it reads
    /// as finished, so the daemon tries again.
    #[must_use]
    pub fn stopped() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
        }
    }

    /// Whether the render thread has stopped on its own (it never should).
    #[must_use]
    pub fn finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Headless {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            if join.join().is_err() {
                eprintln!("kazoo-wall: the render thread panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_formats_round_with_dither_and_hold_full_scale() {
        let mut dither = Dither::new();
        // Full scale and beyond hold to the format's ends.
        assert_eq!(i16::quantise(1.0, &mut dither), i16::MAX);
        assert_eq!(i16::quantise(-4.0, &mut dither), i16::MIN);
        assert_eq!(i32::quantise(1.0, &mut dither), i32::MAX);
        assert_eq!(u16::quantise(-1.0, &mut dither), 0);
        assert!(i16::quantise(f32::NAN, &mut dither).abs() <= 1);
        assert!(f64::quantise(f32::INFINITY, &mut dither).abs() < f64::EPSILON);
        // A value between two steps comes out as either, in proportion,
        // and the average is the value itself: the error is noise, not
        // distortion.
        let value = 100.3 / 32_768.0;
        let samples = 100_000;
        let sum: i64 = (0..samples)
            .map(|_| i64::from(i16::quantise(value as f32, &mut dither)))
            .sum();
        let mean = sum as f64 / f64::from(samples);
        assert!((mean - 100.3).abs() < 0.02, "{mean}");
    }

    #[test]
    fn a_tone_below_one_step_survives_dither() {
        // Without dither a sine of half a step rounds to silence or a square
        // wave; with it the sine is still there on average.
        let mut dither = Dither::new();
        let period = 64;
        let mut folded = vec![0.0_f64; period];
        let cycles = 4_000;
        for n in 0..period * cycles {
            let phase = std::f64::consts::TAU * (n % period) as f64 / period as f64;
            let sample = (0.5 * phase.sin() / 32_768.0) as f32;
            folded[n % period] += f64::from(i16::quantise(sample, &mut dither));
        }
        for (n, total) in folded.iter().enumerate() {
            let expected = 0.5 * (std::f64::consts::TAU * n as f64 / period as f64).sin();
            let got = total / f64::from(u32::try_from(cycles).unwrap());
            assert!((got - expected).abs() < 0.05, "{n}: {got} vs {expected}");
        }
    }

    const RATE: f64 = 48_000.0;

    #[test]
    fn the_timer_renders_when_due_and_waits_otherwise() {
        let start = Instant::now();
        let mut clock = TimerClock::new(start);
        assert_eq!(clock.wait(start, RATE), None, "the first block is due");
        clock.rendered = 4_800;
        // 100 ms rendered, 50 ms gone: wait, but never more than 20 ms at
        // a time.
        assert_eq!(
            clock.wait(start + Duration::from_millis(50), RATE),
            Some(Duration::from_millis(20))
        );
        let nearly = clock.wait(start + Duration::from_millis(95), RATE).unwrap();
        assert!(nearly.abs_diff(Duration::from_millis(5)) < Duration::from_micros(1));
        assert_eq!(clock.wait(start + Duration::from_millis(100), RATE), None);
    }

    #[test]
    fn after_a_stall_the_timer_carries_on_from_now() {
        let start = Instant::now();
        let mut clock = TimerClock::new(start);
        clock.rendered = 256;
        let woken = start + Duration::from_secs(5);
        assert_eq!(clock.wait(woken, RATE), None);
        // Not five seconds of blocks in a rush: the next is a block away.
        clock.rendered += 256;
        let next = clock.wait(woken, RATE).unwrap();
        assert!(
            next.abs_diff(Duration::from_secs_f64(256.0 / RATE)) < Duration::from_micros(1),
            "{next:?}"
        );
    }

    #[test]
    fn the_timer_picks_up_where_the_desk_left_it() {
        let start = Instant::now();
        let mut clock = TimerClock::new(start);
        // The desk paced 3 s of frames over 2 s (it asked for a lead).
        clock.rendered = 144_000;
        let now = start + Duration::from_secs(2);
        clock.follow(now, RATE);
        // Then stopped pacing: the timer neither rushes nor stalls.
        assert_eq!(clock.wait(now, RATE), None);
        clock.rendered += 256;
        let next = clock.wait(now, RATE).unwrap();
        assert!(
            next.abs_diff(Duration::from_secs_f64(256.0 / RATE)) < Duration::from_micros(1),
            "{next:?}"
        );
    }

    #[test]
    fn the_headless_lead_is_forty_milliseconds() {
        assert_eq!(headless_lead_frames(48_000), 1_920);
        assert_eq!(headless_lead_frames(44_100), 1_764);
    }

    /// What the desk says is owed.
    const fn owes(frames: i64, paced_at: Instant) -> DeskOwes {
        DeskOwes {
            frames,
            lead_frames: 2_576,
            paced_at,
        }
    }

    #[test]
    fn a_paced_loop_renders_what_it_owes_and_naps_while_ahead() {
        let now = Instant::now();
        let mut pacer = Pacer::new(now);
        assert_eq!(pacer.step(Some(owes(100, now)), now, RATE), Step::Render);
        assert_eq!(pacer.step(Some(owes(1, now)), now, RATE), Step::Render);
        // Ahead: sleep about as long as it is ahead, within the naps.
        assert_eq!(
            pacer.step(Some(owes(0, now)), now, RATE),
            Step::Sleep(SHORTEST_NAP)
        );
        assert_eq!(
            pacer.step(Some(owes(-48, now)), now, RATE),
            Step::Sleep(Duration::from_millis(1))
        );
        assert_eq!(
            pacer.step(Some(owes(-48_000, now)), now, RATE),
            Step::Sleep(PACED_NAP)
        );
    }

    #[test]
    fn with_no_pace_the_loop_keeps_its_own_time() {
        let now = Instant::now();
        let mut pacer = Pacer::new(now);
        assert_eq!(pacer.step(None, now, RATE), Step::Render);
        pacer.rendered(4_800);
        assert_eq!(
            pacer.step(None, now, RATE),
            Step::Sleep(Duration::from_millis(20))
        );
    }

    #[test]
    fn after_a_long_stall_one_block_goes_and_the_loop_waits_to_hear_where() {
        let at_pace = Instant::now();
        let mut pacer = Pacer::new(at_pace);
        let woken = at_pace + Duration::from_millis(200);
        // 200 ms asleep: the desk played past the wall (owed beyond the
        // lead). One block, not the whole debt.
        assert_eq!(
            pacer.step(Some(owes(12_000, at_pace)), woken, RATE),
            Step::Render
        );
        pacer.rendered(256);
        assert_eq!(
            pacer.step(Some(owes(11_744, at_pace)), woken, RATE),
            Step::Sleep(PACED_NAP)
        );
        // A pace from before the desk placed the stream afresh is not
        // followed either...
        let soon = woken + Duration::from_millis(5);
        assert_eq!(
            pacer.step(Some(owes(11_500, soon)), soon, RATE),
            Step::Sleep(PACED_NAP)
        );
        // ...one from after it is.
        let afresh = woken + REPLACE_SETTLE + Duration::from_millis(1);
        assert_eq!(
            pacer.step(Some(owes(300, afresh)), afresh, RATE),
            Step::Render
        );
        assert_eq!(
            pacer.step(Some(owes(-100, afresh)), afresh, RATE),
            Step::Sleep(PACED_NAP)
        );
    }

    #[test]
    fn a_stall_the_desk_forgave_is_caught_up_after_one_wait() {
        let at_pace = Instant::now();
        let mut pacer = Pacer::new(at_pace);
        // Just past the lead: one block, then wait to hear.
        let woken = at_pace + Duration::from_millis(62);
        assert_eq!(
            pacer.step(Some(owes(2_700, at_pace)), woken, RATE),
            Step::Render
        );
        pacer.rendered(256);
        // The desk took it where it was (it still owes past the lead): the
        // rest goes block after block, with no more waiting.
        let heard = woken + REPLACE_SETTLE + Duration::from_millis(1);
        for owed in [2_900, 2_644, 2_388] {
            assert_eq!(
                pacer.step(Some(owes(owed, heard)), heard, RATE),
                Step::Render
            );
            pacer.rendered(256);
        }
        // Back inside the lead, and a later stall waits once again.
        assert_eq!(
            pacer.step(Some(owes(2_000, heard)), heard, RATE),
            Step::Render
        );
        let later = heard + Duration::from_secs(1);
        assert_eq!(
            pacer.step(Some(owes(9_000, later)), later, RATE),
            Step::Render
        );
        assert_eq!(
            pacer.step(Some(owes(8_744, later)), later, RATE),
            Step::Sleep(PACED_NAP)
        );
    }

    #[test]
    fn a_stall_inside_the_lead_is_caught_up_whole() {
        let at_pace = Instant::now();
        let mut pacer = Pacer::new(at_pace);
        let woken = at_pace + Duration::from_millis(30);
        for owed in [1_440, 1_184, 928, 672, 416, 160] {
            assert_eq!(
                pacer.step(Some(owes(owed, at_pace)), woken, RATE),
                Step::Render
            );
            pacer.rendered(256);
        }
    }

    #[test]
    fn moving_between_the_desk_and_the_timer_neither_bursts_nor_stalls() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let block = Duration::from_secs_f64(256.0 / RATE);
        // Paced for a second: a block owed each block's time, rendered.
        let mut now = start;
        let mut rendered = 0_u64;
        while now < start + Duration::from_secs(1) {
            assert_eq!(pacer.step(Some(owes(256, now)), now, RATE), Step::Render);
            pacer.rendered(256);
            rendered += 256;
            now += block;
        }
        // The desk stops pacing: the next block is due now, as it was on
        // the desk (no stall; a nanosecond's rounding at most)...
        let next = pacer.step(None, now, RATE);
        assert!(
            matches!(next, Step::Render)
                || matches!(next, Step::Sleep(nap) if nap < Duration::from_micros(1)),
            "{next:?} after {rendered} frames"
        );
        pacer.rendered(256);
        // ...and the one after a block's time later (no burst).
        let after = pacer.step(None, now, RATE);
        assert!(
            matches!(after, Step::Sleep(nap) if nap.abs_diff(block) < Duration::from_micros(1)),
            "{after:?}"
        );
        // The desk paces again, the wall a little ahead: it waits.
        let back = now + Duration::from_millis(1);
        assert!(matches!(
            pacer.step(Some(owes(-200, back)), back, RATE),
            Step::Sleep(_)
        ));
    }

    #[test]
    fn a_timer_following_any_count_of_frames_is_never_frozen() {
        let now = Instant::now();
        // However many frames, and whether or not this platform's clock
        // can reach back that far (it counts afresh when it cannot), the
        // next block is due at once after following.
        for rendered in [0, 256, 48_000 * 3_600, u64::MAX / 2, u64::MAX] {
            let mut clock = TimerClock::new(now);
            clock.rendered = rendered;
            clock.follow(now, RATE);
            let wait = clock.wait(now, RATE);
            assert!(
                wait.is_none_or(|nap| nap < Duration::from_micros(1)),
                "{rendered}: {wait:?}"
            );
        }
    }
}
