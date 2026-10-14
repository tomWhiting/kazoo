use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapRb};

use super::*;

struct FakeState {
    default: MicFacts,
    named: MicFacts,
    available: bool,
    fail_open: bool,
    retain_capture: bool,
    faults: VecDeque<MicFault>,
    capture: Arc<Mutex<Option<MicCapture>>>,
    opens: Vec<(u32, u16, usize)>,
}

struct FakeMic(Arc<Mutex<FakeState>>);

struct FakeStream {
    capture: Arc<Mutex<Option<MicCapture>>>,
    retain_capture: bool,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        if !self.retain_capture {
            let capture = self.capture.lock().unwrap().take();
            drop(capture);
        }
    }
}

impl MicBackend for FakeMic {
    type Stream = FakeStream;

    fn facts(&mut self, device: &DeviceChoice) -> Result<MicFacts> {
        let state = self.0.lock().unwrap();
        let facts = match device {
            DeviceChoice::Default => &state.default,
            DeviceChoice::Named(_) => &state.named,
        };
        let result = if state.available {
            Ok(facts.clone())
        } else {
            Err(Error::Device(format!(
                "input device '{}' is unavailable",
                facts.name
            )))
        };
        drop(state);
        result
    }

    fn open(
        &mut self,
        device: &DeviceChoice,
        facts: &MicFacts,
        frames: usize,
        capture: MicCapture,
    ) -> Result<Self::Stream> {
        let mut state = self.0.lock().unwrap();
        if state.fail_open {
            return Err(Error::Device(format!(
                "input device '{}' refused its stream",
                facts.name
            )));
        }
        if let DeviceChoice::Named(name) = device {
            assert_eq!(name, &facts.name);
        }
        let slot = Arc::clone(&state.capture);
        assert!(
            slot.lock().unwrap().is_none(),
            "two writers must never share a ring"
        );
        *slot.lock().unwrap() = Some(capture);
        state.opens.push((facts.rate, facts.channels, frames));
        let retain_capture = state.retain_capture;
        drop(state);
        Ok(FakeStream {
            capture: slot,
            retain_capture,
        })
    }

    fn fault(&mut self, stream: &Self::Stream) -> Option<MicFault> {
        let mut state = self.0.lock().unwrap();
        assert!(Arc::ptr_eq(&stream.capture, &state.capture));
        let fault = state.faults.pop_front();
        drop(state);
        fault
    }
}

struct Rig {
    supervisor: Supervisor<FakeMic>,
    control: Arc<Mutex<FakeState>>,
    consumer: HeapCons<f32>,
    dropped: Arc<AtomicUsize>,
}

impl Rig {
    fn new(
        mic_rate: u32,
        engine_rate: u32,
        channels: u16,
        channel: Option<u16>,
        default: bool,
    ) -> Self {
        let facts = MicFacts {
            name: "Test mic".into(),
            rate: mic_rate,
            channels,
        };
        let control = Arc::new(Mutex::new(FakeState {
            default: facts.clone(),
            named: facts,
            available: true,
            fail_open: false,
            retain_capture: false,
            faults: VecDeque::new(),
            capture: Arc::new(Mutex::new(None)),
            opens: Vec::new(),
        }));
        let (producer, consumer) = HeapRb::<f32>::new(1_000_000).split();
        let dropped = Arc::new(AtomicUsize::new(0));
        let count_dropped = Arc::clone(&dropped);
        let counted: PushCount = Arc::new(move |offered, accepted| {
            count_dropped.fetch_add(offered.saturating_sub(accepted), Ordering::Relaxed);
        });
        let request = MicRequest {
            device: if default {
                DeviceChoice::Default
            } else {
                DeviceChoice::Named("Test mic".into())
            },
            channel,
        };
        let supervisor = Supervisor::new(
            FakeMic(Arc::clone(&control)),
            request,
            MicTarget {
                rate: engine_rate,
                frames: 128,
            },
            producer,
            counted,
            Arc::new(MicShared::default()),
        );
        Self {
            supervisor,
            control,
            consumer,
            dropped,
        }
    }

    fn start(&mut self) {
        self.supervisor.start().unwrap();
    }

    fn feed(&self, data: &[f32], channels: u16, callback_frames: usize) {
        let capture = Arc::clone(&self.control.lock().unwrap().capture);
        let mut slot = capture.lock().unwrap();
        let capture = slot.as_mut().unwrap();
        for pass in data.chunks(callback_frames * usize::from(channels)) {
            capture.process(pass, channels);
        }
        drop(slot);
    }

    fn output(&mut self) -> Vec<f32> {
        assert_eq!(self.dropped.load(Ordering::Relaxed), 0);
        self.consumer.pop_iter().collect()
    }

    fn tolerance(&self) -> usize {
        let capture = Arc::clone(&self.control.lock().unwrap().capture);
        let slot = capture.lock().unwrap();
        let tolerance = slot
            .as_ref()
            .unwrap()
            .resampler
            .as_ref()
            .map_or(0, |r| r.resampler.output_frames_max() + r.delay_frames());
        drop(slot);
        tolerance
    }

    fn fault(&self, fault: MicFault) {
        self.control.lock().unwrap().faults.push_back(fault);
    }

    fn status(&self) -> MicStatus {
        self.supervisor.shared.status()
    }
}

fn sine(hz: f64, rate: u32, seconds: u32) -> Vec<f32> {
    (0..rate * seconds)
        .map(|i| ((std::f64::consts::TAU * hz * f64::from(i) / f64::from(rate)).sin() * 0.5) as f32)
        .collect()
}

fn number(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap())
}

fn rms(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|&x| {
            let x = f64::from(x);
            x * x
        })
        .sum::<f64>()
        / number(samples.len()))
    .sqrt()
}

fn assert_pitch(samples: &[f32], rate: u32, expected: f64) {
    let begin = usize::try_from(rate / 5).unwrap();
    let end = usize::try_from(rate * 9 / 10).unwrap();
    let stable = &samples[begin..end];
    let crossings: Vec<f64> = stable
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
        .map(|(i, pair)| number(i) - f64::from(pair[0]) / f64::from(pair[1] - pair[0]))
        .collect();
    assert!(crossings.len() > 2);
    let measured =
        number(crossings.len() - 1) * f64::from(rate) / (crossings.last().unwrap() - crossings[0]);
    let cents = (1_200.0 * (measured / expected).log2()).abs();
    assert!(
        cents <= 3.0,
        "{measured} Hz instead of {expected} at {rate} Hz: {cents} cents"
    );
}

#[test]
fn equal_rates_pass_bit_for_bit() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let mut seed = 19_u32;
    let input: Vec<f32> = (0..48_000)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f32::from_bits((seed >> 9) | 0x3f80_0000) - 1.5
        })
        .collect();
    rig.feed(&input, 1, 128);
    let output = rig.output();
    assert_eq!(output.len(), input.len());
    assert!(
        output
            .iter()
            .zip(&input)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    let capture = Arc::clone(&rig.control.lock().unwrap().capture);
    assert!(
        capture
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .resampler
            .is_none()
    );
}

#[test]
fn a_24k_mic_reaches_a_48k_engine_at_pitch() {
    let mut rig = Rig::new(24_000, 48_000, 1, None, false);
    rig.start();
    rig.feed(&sine(440.0, 24_000, 1), 1, 64);
    let tolerance = rig.tolerance();
    let output = rig.output();
    assert!(output.len().abs_diff(48_000) <= tolerance);
    assert_pitch(&output, 48_000, 440.0);
    let db = 20.0 * (rms(&output[9_600..43_200]) / (0.5 / 2.0_f64.sqrt())).log10();
    assert!(db.abs() <= 0.5, "amplitude changed by {db} dB");
    assert_eq!(rig.status().resample_errors, 0);
}

#[test]
fn every_rate_from_8k_to_192k_reaches_the_engine() {
    for engine_rate in [48_000, 96_000] {
        for mic_rate in [
            8_000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400,
            192_000,
        ] {
            let hz = if mic_rate == 8_000 { 300.0 } else { 1_000.0 };
            let mut rig = Rig::new(mic_rate, engine_rate, 1, None, false);
            rig.start();
            rig.feed(&sine(hz, mic_rate, 1), 1, 64);
            let tolerance = rig.tolerance();
            let output = rig.output();
            assert!(
                output.len().abs_diff(usize::try_from(engine_rate).unwrap()) <= tolerance,
                "{mic_rate} into {engine_rate}: {} frames, tolerance {tolerance}",
                output.len()
            );
            assert_pitch(&output, engine_rate, hz);
            assert_eq!(rig.status().resample_errors, 0);
        }
    }
}

#[test]
fn a_96k_mic_is_anti_aliased_into_48k() {
    let mut rig = Rig::new(96_000, 48_000, 1, None, false);
    rig.start();
    let input = sine(30_000.0, 96_000, 1);
    rig.feed(&input, 1, 64);
    let output = rig.output();
    let db = 20.0 * (rms(&output[9_600..]) / rms(&input)).log10();
    assert!(
        db <= -60.0,
        "aliased signal is only {db} dB below the input"
    );
}

#[test]
fn rates_outside_8k_to_192k_are_refused_in_words() {
    for rate in [4_000, 384_000] {
        let mut rig = Rig::new(rate, 48_000, 1, None, false);
        let error = rig.supervisor.start().unwrap_err().to_string();
        assert!(error.contains("Test mic"), "{error}");
        assert!(
            error.contains("cannot resample (8000 to 192000)"),
            "{error}"
        );
        assert!(error.contains("kazoo mouth --list-inputs"), "{error}");
        assert!(rig.control.lock().unwrap().opens.is_empty());
    }
}

#[test]
fn the_named_channel_is_the_one_heard() {
    let input = sine(440.0, 48_000, 1);
    let stereo: Vec<f32> = input.iter().flat_map(|&sample| [0.0, sample]).collect();
    for (channel, gain) in [(Some(2), 1.0), (None, 0.5)] {
        let mut rig = Rig::new(48_000, 48_000, 2, channel, false);
        rig.start();
        rig.feed(&stereo, 2, 128);
        let output = rig.output();
        assert_eq!(output.len(), input.len());
        assert!(
            output
                .iter()
                .zip(&input)
                .all(|(out, sample)| (*out - sample * gain).abs() < f32::EPSILON)
        );
        let peak = output.iter().copied().fold(0.0_f32, |a, b| a.max(b.abs()));
        assert!((peak - 0.5 * gain).abs() < 0.001);
    }
    let mut rig = Rig::new(48_000, 48_000, 2, Some(3), false);
    let error = rig.supervisor.start().unwrap_err().to_string();
    assert!(
        error.contains("has 2 channels; channel 3 was asked for"),
        "{error}"
    );
}

#[test]
fn a_lost_device_is_reopened_when_it_returns() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.fault(MicFault::Lost("unplugged".into()));
    rig.supervisor.tick(now + HEALTH_EVERY);
    assert_eq!(rig.status().state, MicState::Lost);
    rig.control.lock().unwrap().available = false;
    for tick in 1..=3 {
        rig.supervisor.tick(now + HEALTH_EVERY + FACTS_EVERY * tick);
        assert_eq!(rig.control.lock().unwrap().opens.len(), 1);
        assert_eq!(rig.status().state, MicState::Lost);
    }
    rig.control.lock().unwrap().available = true;
    rig.supervisor.tick(now + HEALTH_EVERY + FACTS_EVERY * 4);
    assert_eq!(rig.status().state, MicState::Live);
    assert_eq!(rig.status().reopens, 1);
    rig.feed(&[0.25, -0.5], 1, 128);
    assert_eq!(rig.output(), [0.25, -0.5]);
}

#[test]
fn a_rate_change_reopens_at_the_new_rate() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.control.lock().unwrap().named.rate = 24_000;
    rig.supervisor.tick(now + FACTS_EVERY);
    assert_eq!(rig.status().state, MicState::Live);
    assert_eq!(rig.status().device_rate, 24_000);
    assert_eq!(rig.status().reopens, 1);
    assert_eq!(
        rig.control.lock().unwrap().opens,
        [(48_000, 1, 128), (24_000, 1, 64)]
    );
    let slot = Arc::clone(&rig.control.lock().unwrap().capture);
    assert!(slot.lock().unwrap().as_ref().unwrap().resampler.is_some());
    rig.feed(&sine(440.0, 24_000, 1), 1, 64);
    assert_pitch(&rig.output(), 48_000, 440.0);
}

#[test]
fn the_default_moving_to_another_device_is_followed() {
    for default in [true, false] {
        let mut rig = Rig::new(48_000, 48_000, 1, None, default);
        rig.start();
        let now = rig.supervisor.last_facts;
        rig.control.lock().unwrap().default.name = "Replacement mic".into();
        rig.supervisor.tick(now + FACTS_EVERY);
        assert_eq!(rig.status().reopens, u32::from(default));
        assert_eq!(
            rig.control.lock().unwrap().opens.len(),
            if default { 2 } else { 1 }
        );
        assert_eq!(
            rig.status().device.as_str(),
            if default {
                "Replacement mic"
            } else {
                "Test mic"
            }
        );
    }
}

#[test]
fn a_glitch_is_not_a_loss() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.fault(MicFault::Glitch);
    rig.fault(MicFault::Glitch);
    rig.supervisor.tick(now + HEALTH_EVERY);
    assert_eq!(rig.status().state, MicState::Live);
    assert_eq!(rig.status().glitches, 2);
    assert_eq!(rig.status().reopens, 0);
    assert_eq!(rig.control.lock().unwrap().opens.len(), 1);
}

#[test]
fn a_stream_that_never_lets_go_is_said() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.control.lock().unwrap().retain_capture = true;
    rig.supervisor.release_wait = Duration::ZERO;
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.fault(MicFault::Lost("unplugged".into()));
    rig.supervisor.tick(now + HEALTH_EVERY);
    assert_eq!(rig.status().state, MicState::Stuck);
    rig.supervisor.tick(now + FACTS_EVERY * 10);
    assert_eq!(rig.control.lock().unwrap().opens.len(), 1);
    assert!(
        rig.supervisor
            .last_error
            .as_ref()
            .unwrap()
            .contains("did not release")
    );
    // The retained capture is released at test teardown, without leaking memory.
}

#[test]
fn capture_never_allocates() {
    for (mic_rate, channels, channel) in [
        (24_000, 1, None),
        (48_000, 1, None),
        (24_000, 2, Some(2)),
        (48_000, 2, Some(2)),
        (24_000, 2, None),
    ] {
        let mut rig = Rig::new(mic_rate, 48_000, channels, channel, false);
        rig.start();
        let mono = sine(440.0, mic_rate, 10);
        let input = if channels == 2 {
            mono.iter().flat_map(|&x| [0.0, x]).collect()
        } else {
            mono
        };
        let slot = Arc::clone(&rig.control.lock().unwrap().capture);
        let mut slot = slot.lock().unwrap();
        let capture = slot.as_mut().unwrap();
        let before = assert_no_alloc::violation_count();
        assert_no_alloc::assert_no_alloc(|| {
            for pass in input.chunks(64 * usize::from(channels)) {
                capture.process(pass, channels);
            }
        });
        drop(slot);
        assert_eq!(assert_no_alloc::violation_count(), before);
        assert_eq!(rig.status().resample_errors, 0);
        assert_eq!(rig.dropped.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn nonsense_samples_reach_the_engine_as_silence() {
    let mut rig = Rig::new(24_000, 48_000, 1, None, false);
    rig.start();
    let nonsense: Vec<f32> = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30]
        .into_iter()
        .cycle()
        .take(320)
        .collect();
    rig.feed(&nonsense, 1, 64);
    let silence = rig.output();
    assert!(
        silence
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= f32::EPSILON)
    );
    rig.feed(&sine(440.0, 24_000, 1), 1, 64);
    let output = rig.output();
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_pitch(&output, 48_000, 440.0);
}

#[test]
fn the_status_names_the_mic() {
    let name = format!("{}é{}", "a".repeat(47), "b".repeat(21));
    assert_eq!(name.len(), 70);
    let short = MicName::new(&name);
    assert!(short.as_str().len() <= NAME_BYTES);
    assert_eq!(short.as_str(), "a".repeat(47));
    assert_eq!(MicName::new("AirPods Pro").as_str(), "AirPods Pro");
    assert_eq!(MicName::default().as_str(), "");
}

#[test]
fn capture_accepts_device_callbacks_larger_than_scratch() {
    let mono = sine(440.0, 48_000, 1);
    let stereo: Vec<f32> = mono.iter().flat_map(|&x| [0.0, x]).collect();
    for channel in [None, Some(2)] {
        let mut rig = Rig::new(48_000, 48_000, 2, channel, false);
        rig.start();
        rig.feed(&stereo, 2, 10_000);
        let gain = if channel.is_some() { 1.0 } else { 0.5 };
        let output = rig.output();
        assert_eq!(output.len(), mono.len());
        assert!(
            output
                .iter()
                .zip(&mono)
                .all(|(a, b)| (*a - b * gain).abs() < f32::EPSILON)
        );
    }
}

#[test]
fn first_facts_errors_name_the_device_and_list_command() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.control.lock().unwrap().available = false;
    let error = rig.supervisor.start().unwrap_err().to_string();
    assert!(error.contains("Test mic"), "{error}");
    assert!(
        error.contains("kazoo mouth --list-inputs lists the inputs"),
        "{error}"
    );
}

#[test]
fn a_failed_reopen_keeps_the_producer_for_the_next_attempt() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.fault(MicFault::Lost("unplugged".into()));
    rig.supervisor.tick(now + HEALTH_EVERY);
    rig.control.lock().unwrap().fail_open = true;
    rig.supervisor.tick(now + HEALTH_EVERY + FACTS_EVERY);
    assert_eq!(rig.status().state, MicState::Lost);
    assert!(rig.supervisor.sink.is_some());
    rig.control.lock().unwrap().fail_open = false;
    rig.supervisor.tick(now + HEALTH_EVERY + FACTS_EVERY * 2);
    assert_eq!(rig.status().state, MicState::Live);
    assert_eq!(rig.status().reopens, 1);
    rig.feed(&[0.5], 1, 128);
    assert_eq!(rig.output(), [0.5]);
}

#[test]
fn callback_frames_and_resampler_chunks_are_rounded_and_bounded() {
    assert_eq!(rounded_frames(1, 44_100, 1_000, 32, 256).unwrap(), 44);
    assert_eq!(rounded_frames(1, 8_000, 1_000, 32, 256).unwrap(), 32);
    assert_eq!(rounded_frames(1, 192_000, 1_000, 32, 256).unwrap(), 192);
    assert_eq!(rounded_frames(128, 24_000, 48_000, 32, 1_024).unwrap(), 64);
    assert_eq!(rounded_frames(128, 8_000, 192_000, 32, 1_024).unwrap(), 32);
    assert_eq!(
        rounded_frames(1_024, 192_000, 8_000, 32, 1_024).unwrap(),
        1_024
    );
    assert!(rounded_frames(1, 24_000, 0, 32, 256).is_err());
}

#[test]
fn default_devices_with_the_same_status_prefix_are_distinguished() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, true);
    let prefix = "a".repeat(NAME_BYTES);
    rig.control.lock().unwrap().default.name = format!("{prefix} first");
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.control.lock().unwrap().default.name = format!("{prefix} second");
    rig.supervisor.tick(now + FACTS_EVERY);
    assert_eq!(rig.status().reopens, 1);
    assert_eq!(rig.control.lock().unwrap().opens.len(), 2);
}

#[test]
fn a_channel_count_change_reopens_at_the_native_width() {
    let mut rig = Rig::new(48_000, 48_000, 1, None, false);
    rig.start();
    let now = rig.supervisor.last_facts;
    rig.control.lock().unwrap().named.channels = 2;
    rig.supervisor.tick(now + FACTS_EVERY);
    assert_eq!(rig.status().device_channels, 2);
    assert_eq!(rig.status().reopens, 1);
    rig.feed(&[0.0, 0.5], 2, 128);
    assert_eq!(rig.output(), [0.25]);
}
