use super::super::audio::{Chosen, OutputInfo};
use super::super::audio_open::apply_fallback;
use super::*;
use std::f32::consts::TAU;

const RATE: u32 = 48_000;
const BLOCK: usize = 64;

fn sine(frame: u64, hz: f32) -> f32 {
    0.5 * (TAU * hz * frame as f32 / RATE as f32).sin()
}

fn fill(start: u64, out: &mut [f32], hz: f32) {
    for (offset, sample) in out.iter_mut().enumerate() {
        *sample = sine(start + offset as u64, hz);
    }
}

#[test]
fn in_before_out_adds_nothing() {
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    let mut input = [0.0; BLOCK];
    let mut output = [0.0; BLOCK];
    for cycle in 0..7500_u64 {
        fill(cycle * BLOCK as u64, &mut input, 100.0);
        writer.write(&input);
        assert_ne!(reader.read(&mut output), ReadReport::Priming);
        if cycle > 0 {
            assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
        }
    }
    assert_eq!(reader.slack(), 0);
    assert_eq!(reader.latency_frames(), 128);
    assert_eq!(reader.stats().underruns, 0);
    assert_eq!(reader.stats().jumps, 0);
}

#[test]
fn out_before_in_costs_one_buffer_and_never_drops() {
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    let mut input = [0.0; BLOCK];
    let mut previous = input;
    let mut output = input;
    for cycle in 0..7500_u64 {
        let report = reader.read(&mut output);
        if cycle == 0 {
            assert_eq!(report, ReadReport::Priming);
        } else if cycle > 1 {
            assert_eq!(previous.map(f32::to_bits), output.map(f32::to_bits));
        }
        fill(cycle * BLOCK as u64, &mut input, 100.0);
        writer.write(&input);
        previous = input;
    }
    assert_eq!(reader.stats().underruns, 0);
}

struct Schedule {
    writer: LiveWriter,
    reader: LiveInput,
    captured: u64,
    input_cycle: u64,
    next_input: i64,
    seed: u64,
    ppm: i64,
    jitter: bool,
}

impl Schedule {
    fn new(ppm: i64, jitter: bool) -> Self {
        let (writer, reader) = live_pair(RATE, BLOCK);
        Self {
            writer,
            reader,
            captured: 0,
            input_cycle: 0,
            next_input: 0,
            seed: 17,
            ppm,
            jitter,
        }
    }

    fn advance(&mut self, cycle: u64, signal: impl Fn(u64) -> f32) -> [f32; BLOCK] {
        let now = i64::try_from(cycle).unwrap() * 64_000_000;
        while self.next_input <= now {
            let mut input = [0.0; BLOCK];
            for (offset, sample) in input.iter_mut().enumerate() {
                *sample = signal(self.captured + offset as u64);
            }
            self.writer.write(&input);
            self.captured += BLOCK as u64;
            self.input_cycle += 1;
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let jitter = if self.jitter {
                i64::try_from((self.seed >> 32) % 38_400_001).unwrap() - 19_200_000
            } else {
                0
            };
            self.next_input = i64::try_from(self.input_cycle).unwrap() * 64_000_000 * 1_000_000
                / (1_000_000 + self.ppm)
                + jitter;
        }
        let mut output = [0.0; BLOCK];
        self.reader.read(&mut output);
        output
    }
}

fn assert_no_holes(samples: &[f32; BLOCK]) {
    assert!(
        !samples
            .windows(8)
            .any(|window| window.iter().all(|x| *x == 0.0))
    );
}

#[test]
fn jitter_grows_the_slack_and_then_holds() {
    let mut schedule = Schedule::new(0, true);
    let mut settled = 0;
    for cycle in 0..7500 {
        let output = schedule.advance(cycle, |frame| sine(frame, 100.0));
        if cycle > 750 {
            assert_no_holes(&output);
        }
        if cycle == 750 {
            settled = schedule.reader.stats().underruns;
        }
    }
    assert!(settled > 0);
    assert_eq!(schedule.reader.stats().underruns, settled);
    assert!(schedule.reader.slack() <= BLOCK);
    assert_eq!(schedule.reader.slack() % SLACK_STEP, 0);
}

#[test]
fn a_fast_input_clock_is_pulled_back() {
    let mut schedule = Schedule::new(100, false);
    let mut late_frames = 0;
    for cycle in 0..15_000 {
        schedule.advance(cycle, |frame| sine(frame, 100.0));
        let lag = schedule.reader.lag();
        let target = BLOCK + schedule.reader.slack();
        if lag > (target + BLOCK / 2 + 5) as u64 {
            late_frames += BLOCK;
        } else {
            late_frames = 0;
        }
        assert!(late_frames <= RATE as usize / 4);
    }
    assert!((1..=4).contains(&schedule.reader.stats().jumps));
}

#[test]
fn a_slow_input_clock_repeats_instead_of_going_silent() {
    let mut schedule = Schedule::new(-100, false);
    let mut previous = 0.0_f32;
    for cycle in 0..15_000 {
        let output = schedule.advance(cycle, |frame| sine(frame, 100.0));
        if cycle > 1 {
            assert_no_holes(&output);
            for sample in output {
                assert!(
                    (sample - previous).abs() <= 0.06,
                    "step {}",
                    sample - previous
                );
                previous = sample;
            }
        } else {
            previous = output[BLOCK - 1];
        }
    }
    assert!(schedule.reader.stats().underruns > 0);
}

fn musical(frame: u64) -> f32 {
    let time = frame as f64 / f64::from(RATE);
    let phase = if time < 2.0 {
        220.0_f64.mul_add(time, (293.66 - 220.0) * time * time / 4.0)
    } else {
        let t = time - 2.0;
        let modulation = 220.0 * ((30.0_f64 / 1200.0).exp2() - 1.0);
        220.0_f64.mul_add(t, 513.66)
            + modulation * (1.0 - (f64::from(TAU) * 6.0 * t).cos()) / (f64::from(TAU) * 6.0)
    };
    (0.5 * (f64::from(TAU) * phase).sin()) as f32
}

fn pitch(window: &[f32]) -> Option<f64> {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0_u32;
    for (index, pair) in window.windows(2).enumerate() {
        if pair[0] <= 0.0 && pair[1] > 0.0 {
            let crossing = index as f64 - f64::from(pair[0]) / f64::from(pair[1] - pair[0]);
            if first.is_none() {
                first = Some(crossing);
            }
            last = crossing;
            count += 1;
        }
    }
    first
        .filter(|_| count >= 2)
        .map(|start| f64::from(RATE) * f64::from(count - 1) / (last - start))
}

#[test]
fn a_glide_and_a_vibrato_come_through_without_steps() {
    for ppm in [100, -100] {
        let mut schedule = Schedule::new(ppm, false);
        let mut heard = Vec::new();
        let mut events = Vec::new();
        let mut reference = Vec::new();
        for cycle in 0..3000 {
            let before = schedule.reader.stats();
            heard.extend(schedule.advance(cycle, musical));
            let start = schedule.reader.position().saturating_sub(BLOCK as u64);
            reference.extend((0..BLOCK).map(|i| musical(start + i as u64)));
            let after = schedule.reader.stats();
            if after.jumps != before.jumps || after.underruns != before.underruns {
                events.push(cycle as usize * BLOCK);
            }
        }
        let mut previous: Option<(f64, f64, bool)> = None;
        for (index, window) in heard.chunks_exact(480).enumerate().skip(2) {
            let start = index * 480;
            if let (Some(actual), Some(expected)) =
                (pitch(window), pitch(&reference[start..start + 480]))
            {
                let jumped = events.iter().any(|event| event.abs_diff(start) < 480);
                let tolerance = if jumped { 20.0 } else { 3.0 };
                let cents = (1200.0 * (actual / expected).log2()).abs();
                assert!(
                    cents < tolerance,
                    "{ppm} ppm, window {index}: {cents} cents"
                );
                if let Some((old_actual, old_expected, old_jump)) = previous {
                    if !jumped && !old_jump {
                        let change = (1200.0 * (actual / old_actual).log2()).abs();
                        let natural = (1200.0 * (expected / old_expected).log2()).abs();
                        assert!(change <= natural + 2.0);
                    }
                }
                previous = Some((actual, expected, jumped));
            }
        }
    }
}

fn capture(channels: &[u16], count: u16) -> (InputCapture, Vec<LiveInput>) {
    let (writers, readers) = channels.iter().map(|_| live_pair(RATE, BLOCK)).unzip();
    (
        InputCapture::new(channels.to_vec(), count, writers),
        readers,
    )
}

#[test]
fn the_channel_asked_for_is_the_one_heard() {
    let (mut callback, mut readers) = capture(&[3], 4);
    let mut input = [0.0; BLOCK * 4];
    for cycle in 0..3_u64 {
        for (index, frame) in input.chunks_exact_mut(4).enumerate() {
            frame.copy_from_slice(&[0.8, -0.7, sine(cycle * 64 + index as u64, 220.0), 0.3]);
        }
        callback.process(&input);
        let mut output = [0.0; BLOCK];
        readers[0].read(&mut output);
        if cycle > 0 {
            for (sample, frame) in output.iter().zip(input.chunks_exact(4)) {
                assert_eq!(sample.to_bits(), frame[2].to_bits());
            }
        }
    }
    let mut burst = vec![0.25_f32; 5000 * 4];
    burst[2] = f32::NAN;
    burst[6] = f32::INFINITY;
    burst[10] = f32::NEG_INFINITY;
    burst[4999 * 4 + 2] = 1e30;
    let allocations = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| callback.process(&burst));
    assert_eq!(assert_no_alloc::violation_count(), allocations);
    let stats = readers[0].stats();
    assert_eq!(stats.in_callback_max, 5000);
    assert_eq!(stats.nonfinite, 3);
    assert_eq!(stats.raw_clips, 1);
    assert_eq!(stats.raw_peak.to_bits(), 1e30_f32.to_bits());
    let mut output = [0.0; BLOCK];
    readers[0].read(&mut output);
    assert!(
        output
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
    );
    assert_eq!(output[BLOCK - 1].to_bits(), 1.0_f32.to_bits());
}

#[test]
fn two_channels_read_independently() {
    let (mut callback, mut readers) = capture(&[1, 2], 2);
    let mut input = [0.0; BLOCK * 2];
    for cycle in 0..3_u64 {
        for (index, frame) in input.chunks_exact_mut(2).enumerate() {
            frame[0] = sine(cycle * 64 + index as u64, 100.0);
            frame[1] = sine(cycle * 64 + index as u64, 440.0);
        }
        callback.process(&input);
        for (channel, reader) in readers.iter_mut().enumerate() {
            let mut output = [0.0; BLOCK];
            reader.read(&mut output);
            if cycle > 0 {
                for (sample, frame) in output.iter().zip(input.chunks_exact(2)) {
                    assert_eq!(sample.to_bits(), frame[channel].to_bits());
                }
            }
        }
    }
}

#[test]
fn raw_clipping_is_counted_before_any_gain() {
    let (mut writer, reader) = live_pair(RATE, BLOCK);
    writer.write(&[0.9995, -1.0, 0.998]);
    assert_eq!(reader.stats().raw_clips, 2);
    assert_eq!(reader.stats().raw_peak.to_bits(), 1.0_f32.to_bits());
}

#[test]
fn nonsense_never_leaves_the_ring() {
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    writer.write(&[0.5; BLOCK]);
    reader.read(&mut [0.0; BLOCK]);
    writer.write(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30]);
    let mut output = [0.0; 4];
    reader.read(&mut output);
    assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert_eq!(reader.stats().nonfinite, 3);
    assert_eq!(reader.stats().raw_clips, 1);
    assert_eq!(output[3].to_bits(), 1.0_f32.to_bits());
    assert_eq!(reader.stats().raw_peak.to_bits(), 1e30_f32.to_bits());
}

#[test]
fn a_stalled_reader_is_an_overrun_not_a_torn_read() {
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    writer.write(&[0.2; BLOCK]);
    reader.read(&mut [0.0; BLOCK]);
    writer.write(&vec![0.7; 9600]);
    let mut output = [0.0; BLOCK];
    assert_eq!(reader.read(&mut output), ReadReport::Overrun);
    assert_eq!(reader.stats().overruns, 1);
    assert!(output.iter().all(|sample| *sample >= 0.0 && *sample <= 0.7));
    assert_eq!(output[BLOCK - 1].to_bits(), 0.7_f32.to_bits());
}

#[test]
fn the_callbacks_never_allocate() {
    let mut schedule = Schedule::new(0, true);
    let (mut capture, readers) = capture(&[3], 4);
    let mut meter = RoundTrip::new(RATE);
    let mut played = [0.0; BLOCK];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for cycle in 0..7500 {
            let heard = schedule.advance(cycle, |frame| sine(frame, 100.0));
            capture.process(&[0.1_f32; BLOCK * 4]);
            meter.play(&mut played);
            meter.hear(&heard);
        }
    });
    assert_eq!(readers[0].stats().captured, 480_000);
    assert_eq!(assert_no_alloc::violation_count(), before);
}

#[test]
fn a_refused_fixed_buffer_is_reported() {
    let mut chosen = Chosen {
        config: cpal::StreamConfig {
            channels: 2,
            sample_rate: RATE,
            buffer_size: cpal::BufferSize::Fixed(64),
        },
        format: cpal::SampleFormat::F32,
        info: OutputInfo {
            channels: 2,
            rate: RATE,
            frames: BLOCK,
        },
    };
    assert!(!apply_fallback(&mut chosen, false));
    assert_eq!(chosen.config.buffer_size, cpal::BufferSize::Fixed(64));
    assert!(apply_fallback(&mut chosen, true));
    assert_eq!(chosen.config.buffer_size, cpal::BufferSize::Default);
    assert_eq!(chosen.info.frames, BLOCK);
}

#[test]
fn a_channel_beyond_the_device_is_refused_with_its_count() {
    let error = check_channels("interface", 2, &[3]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("input device 'interface' has 2 channels; channel 3 was asked for")
    );
}

fn round_trip(
    delay: usize,
    connected: bool,
) -> std::result::Result<RoundTripResult, RoundTripError> {
    let mut meter = RoundTrip::new(RATE);
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    let mut cable = vec![0.0; delay.max(1)];
    let mut position = 0;
    let mut played = [0.0; BLOCK];
    let mut input = [0.0; BLOCK];
    let mut heard = [0.0; BLOCK];
    for cycle in 0..3400 {
        meter.play(&mut played);
        for (index, (target, source)) in input.iter_mut().zip(played).enumerate() {
            *target = if connected { cable[position] } else { 0.0 };
            *target += if (cycle * BLOCK + index) % 2 == 0 {
                0.001
            } else {
                -0.001
            };
            cable[position] = source;
            position = (position + 1) % cable.len();
        }
        writer.write(&input);
        reader.read(&mut heard);
        meter.hear(&heard);
        if let Some(result) = meter.result() {
            return result;
        }
    }
    panic!("round trip did not finish");
}

#[test]
fn the_round_trip_finds_a_known_delay() {
    let result = round_trip(317, true).unwrap();
    assert!(result.frames.abs_diff(317) <= 1, "{result:?}");
    assert!(result.spread <= 2);
}

#[test]
fn the_round_trip_says_when_nothing_comes_back() {
    assert_eq!(round_trip(317, false), Err(RoundTripError::NothingCameBack));
}

#[test]
fn profiles_parse_and_say_their_ranges() {
    for profile in InputProfile::ALL {
        assert_eq!(InputProfile::parse(profile.name()), Some(profile));
    }
    assert_eq!(InputProfile::Bass.range_hz(), (29.0, 500.0));
    assert_eq!(InputProfile::parse("unknown"), None);
}

fn largest_step(previous: f32, block: &[f32]) -> f32 {
    block
        .iter()
        .scan(previous, |last, &sample| {
            let step = (sample - *last).abs();
            *last = sample;
            Some(step)
        })
        .fold(0.0, f32::max)
}

#[test]
fn an_underrun_with_a_short_lag_still_crossfades_for_a_millisecond() {
    let (mut writer, mut reader) = live_pair(RATE, BLOCK);
    let mut input = [0.0; BLOCK];
    let mut output = [0.0; BLOCK];
    let mut written = 0_u64;
    for _ in 0..20 {
        fill(written, &mut input, 400.0);
        writer.write(&input);
        written += BLOCK as u64;
        reader.read(&mut output);
    }
    let previous = output[BLOCK - 1];
    let mut short = [0.0; 8];
    fill(written, &mut short, 400.0);
    writer.write(&short);
    assert_eq!(reader.read(&mut output), ReadReport::Underrun);
    // A 0.5 sine at 400 Hz moves at most 0.027 a sample; a 1 ms crossfade
    // between two points of it adds at most 1.0 / 48 more.
    let step = largest_step(previous, &output);
    assert!(step <= 0.05, "largest step {step}");
}
